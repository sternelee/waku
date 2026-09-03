//! The mobile app's state entity: navigation, the remote connection, the
//! shared session catalog, and the open chat with its live event projection.
//!
//! Everything user-visible renders from this one entity. Background work
//! (dialing, requests) runs on GPUI's background executor against cloned
//! `DaemonClient` handles; results land back through `entity.update`.

use std::time::Duration;

use crossbeam_channel::Receiver;
use gpui::{prelude::*, App, Context, Entity};
use uuid::Uuid;
use waku_client::{
    DaemonClient, DaemonSupervisor, SequencedEvent,
};
use waku_client::model::{
    AgentSession, DriverEvent, Message, MessageRole, PermissionOption, RuntimeEventCursor,
    SessionStatus, TurnStatus, unix_time,
};

use crate::remote;
use crate::screens::Screen;

// ── Persistence (shared_preferences) ────────────────────────────────────────

const TICKET_KEY: &str = "waku.remote_ticket";

fn load_saved_ticket() -> Option<String> {
    let prefs = gpui_mobile::packages::shared_preferences::SharedPreferences::instance();
    prefs
        .get_string(TICKET_KEY)
        .filter(|ticket| !ticket.is_empty())
}

fn save_ticket(ticket: Option<&str>) {
    let prefs = gpui_mobile::packages::shared_preferences::SharedPreferences::instance();
    match ticket.filter(|ticket| !ticket.is_empty()) {
        Some(ticket) => {
            let _ = prefs.set_string(TICKET_KEY, ticket);
        }
        None => {
            let _ = prefs.remove(TICKET_KEY);
        }
    }
}

// ── Connection ──────────────────────────────────────────────────────────────

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ConnectionPhase {
    Disconnected,
    Connecting,
    Connected,
}

// ── Chat projection ─────────────────────────────────────────────────────────

/// The open chat. Its `session` snapshot starts from `HydrateSession` and then
/// advances purely through driver events — the authoritative daemon transcript
/// is only re-read when the chat reopens.
pub struct ChatSession {
    pub session: AgentSession,
    pub runtime_id: Option<Uuid>,
    pub supports_steer: bool,
    pub events: Option<Receiver<SequencedEvent>>,
    pub cursor: Option<RuntimeEventCursor>,
    /// Title of the newest activity in the running turn, shown under the
    /// working indicator.
    pub activity: Option<String>,
    pub pending_permission: Option<PendingPermission>,
    /// Whether a prompt send is waiting on the runtime to come up.
    pub starting: bool,
    pub error: Option<String>,
}

pub struct PendingPermission {
    pub request_id: String,
    pub title: String,
    pub detail: String,
    pub options: Vec<PermissionOption>,
}

impl ChatSession {
    fn status(&self) -> SessionStatus {
        self.session.status
    }

    pub fn is_busy(&self) -> bool {
        matches!(
            self.status(),
            SessionStatus::Connecting | SessionStatus::Working | SessionStatus::Waiting
        )
    }

    /// The assistant message currently streaming, if any.
    fn streaming_message_mut(&mut self) -> Option<&mut Message> {
        let active = self.session.turns.last().map(|turn| turn.id);
        self.session
            .messages
            .iter_mut()
            .rev()
            .find(|message| {
                message.role == MessageRole::Assistant
                    && (message.streaming
                        || (active.is_some_and(|turn| message.turn_id == Some(turn))
                            && !message.content.is_empty()))
            })
    }
}

// ── App ─────────────────────────────────────────────────────────────────────

thread_local! {
    /// Pending IME text delivered by the platform keyboard callback between
    /// frames. `render` drains it so it can mutate the entity.
    static PENDING_IME: std::cell::RefCell<Vec<String>> = std::cell::RefCell::new(Vec::new());
}

/// The platform keyboard callback (installed at startup) — runs on the UI
/// thread inside the frame callback, so buffering and letting `render` drain
/// is the only safe way to mutate the entity.
pub fn install_ime_callback(entity: Entity<WakuMobile>) {
    gpui_mobile::set_text_input_callback(Some(Box::new(move |text: &str| {
        PENDING_IME.with(|buffer| buffer.borrow_mut().push(text.to_owned()));
        let _ = &entity;
    })));
}

pub struct WakuMobile {
    pub screen: Screen,
    history: Vec<Screen>,
    pub safe_area: crate::screens::SafeArea,

    pub ticket_input: String,
    pub active_field: crate::screens::connect::FieldTarget,
    pub compose_input: String,

    pub phase: ConnectionPhase,
    pub daemon_label: Option<String>,
    pub error: Option<String>,
    pub saved_ticket: Option<String>,

    supervisor: Option<DaemonSupervisor>,
    pub client: Option<DaemonClient>,
    pub sessions: Vec<AgentSession>,
    pub loading_catalog: bool,

    pub chat: Option<ChatSession>,
    pub chat_loading: bool,

    pub chat_scroll: gpui::ScrollHandle,

    pub toast: Option<String>,
}

impl WakuMobile {
    pub fn new(cx: &mut App) -> Entity<Self> {
        let safe_area = crate::screens::query_safe_area();
        let saved_ticket = load_saved_ticket();
        cx.new(|_| Self {
            screen: Screen::default(),
            history: Vec::new(),
            safe_area,
            ticket_input: saved_ticket.clone().unwrap_or_default(),
            active_field: crate::screens::connect::FieldTarget::Ticket,
            compose_input: String::new(),
            phase: ConnectionPhase::Disconnected,
            daemon_label: None,
            error: None,
            saved_ticket,
            supervisor: None,
            client: None,
            sessions: Vec::new(),
            loading_catalog: false,
            chat: None,
            chat_loading: false,
            chat_scroll: gpui::ScrollHandle::default(),
            toast: None,
        })
    }

    /// Reconnect the persisted ticket at launch, silently.
    pub fn reconnect_saved(&mut self, cx: &mut Context<Self>) {
        let Some(ticket) = self.saved_ticket.clone() else {
            return;
        };
        self.connect(ticket, true, cx);
    }

    // ── Connection ──

    pub fn connect(&mut self, ticket: String, persist: bool, cx: &mut Context<Self>) {
        if self.phase == ConnectionPhase::Connecting {
            return;
        }
        self.phase = ConnectionPhase::Connecting;
        self.error = None;
        cx.notify();
        let task = cx.background_executor().spawn(async move {
            remote::connect(&ticket).map(|(supervisor, client, label)| {
                (supervisor, client, label, ticket)
            })
        });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, move |this, cx| {
                this.phase = ConnectionPhase::Disconnected;
                match result {
                    Ok((supervisor, client, label, ticket)) => {
                        this.supervisor = Some(supervisor);
                        this.client = Some(client.clone());
                        this.daemon_label = Some(label);
                        this.phase = ConnectionPhase::Connected;
                        this.saved_ticket = Some(ticket.clone());
                        this.ticket_input = ticket;
                        if persist {
                            save_ticket(Some(&this.ticket_input));
                        }
                        this.error = None;
                        this.loading_catalog = true;
                        this.refresh_catalog(&client, cx);
                        this.start_polling(cx);
                        if this.screen == Screen::Connect {
                            this.navigate_to(Screen::Sessions);
                        }
                    }
                    Err(error) => {
                        this.error = Some(error.to_string());
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    pub fn disconnect(&mut self, cx: &mut Context<Self>) {
        self.close_chat(cx);
        if let Some(client) = self.client.take() {
            client.shutdown();
        }
        self.supervisor = None;
        self.sessions.clear();
        self.phase = ConnectionPhase::Disconnected;
        self.daemon_label = None;
        self.saved_ticket = None;
        save_ticket(None);
        self.navigate_to(Screen::Connect);
        cx.notify();
    }

    // ── Catalog ──

    /// Reload the shared session catalog off the UI thread.
    pub fn refresh_catalog(&mut self, client: &DaemonClient, cx: &mut Context<Self>) {
        let client = client.clone();
        let task = cx.background_executor().spawn(async move {
            remote::load_catalog(&client).map_err(|error| error.to_string())
        });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, move |this, cx| {
                this.loading_catalog = false;
                match result {
                    Ok(catalog) => {
                        this.sessions = catalog.sessions;
                    }
                    Err(error) => {
                        this.error = Some(error);
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    // ── Chat lifecycle ──

    pub fn open_chat(&mut self, session_id: Uuid, cx: &mut Context<Self>) {
        let Some(client) = self.client.clone() else {
            return;
        };
        self.chat_loading = true;
        self.chat = None;
        self.navigate_to(Screen::Chat);
        cx.notify();
        let task = cx.background_executor().spawn(async move {
            remote::hydrate(&client, session_id).map_err(|error| error.to_string()).and_then(
                |session| {
                    // A session whose runtime cannot come up still opens
                    // read-only; the reason surfaces as a hint in the chat.
                    match remote::ensure_runtime(&client, &session) {
                        Ok(runtime) => Ok((session, Some(runtime))),
                        Err(_) => Ok((session, None)),
                    }
                },
            )
        });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, move |this, cx| {
                this.chat_loading = false;
                match result {
                    Ok((session, runtime)) => {
                        let runtime_id = runtime.as_ref().map(|runtime| runtime.runtime_id);
                        let supports_steer =
                            runtime.as_ref().is_some_and(|runtime| runtime.supports_steer);
                        let mut chat = ChatSession {
                            session,
                            runtime_id,
                            supports_steer,
                            events: None,
                            cursor: None,
                            activity: None,
                            pending_permission: None,
                            starting: false,
                            error: None,
                        };
                        if let (Some(runtime_id), Some(client)) = (runtime_id, this.client.clone())
                        {
                            chat.events = Some(client.subscribe(chat.session.id, runtime_id));
                        }
                        if runtime.is_none() {
                            chat.error = Some("远程主机上没有可附着的运行时；发送消息时会自动启动".into());
                        }
                        this.chat = Some(chat);
                        this.start_chat_polling(cx);
                    }
                    Err(error) => {
                        this.toast = Some(error);
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    pub fn close_chat(&mut self, cx: &mut Context<Self>) {
        if let Some(chat) = self.chat.as_ref()
            && let Some(client) = self.client.as_ref()
        {
            client.unsubscribe(chat.session.id, chat.runtime_id.unwrap_or_default());
        }
        self.chat = None;
        self.compose_input.clear();
        self.active_field = crate::screens::connect::FieldTarget::Ticket;
        gpui_mobile::hide_keyboard();
        let _ = cx;
    }

    // ── Composer actions ──

    pub fn send_composer(&mut self, cx: &mut Context<Self>) {
        let prompt = self.compose_input.trim().to_owned();
        if prompt.is_empty() {
            return;
        }
        let Some(chat) = self.chat.as_mut() else {
            return;
        };
        if chat.is_busy() {
            // The running turn either accepts steering or the send is
            // refused; the UI shows Stop in that state.
            return;
        }
        self.compose_input.clear();
        self.active_field = crate::screens::connect::FieldTarget::Ticket;
        gpui_mobile::hide_keyboard();
        let client = match self.client.clone() {
            Some(client) => client,
            None => return,
        };

        // Optimistic transcript: the user bubble lands now, the turn starts
        // when the daemon confirms.
        let turn_id = Uuid::new_v4();
        let prompt_message = Message {
            id: Uuid::new_v4(),
            turn_id: Some(turn_id),
            role: MessageRole::User,
            display_content: None,
            content: prompt.clone(),
            attachments: Vec::new(),
            created_at: unix_time(),
            streaming: false,
        };
        chat.session.messages.push(prompt_message);
        chat.session.status = SessionStatus::Connecting;
        chat.error = None;
        chat.activity = None;
        cx.notify();

        let session_id = chat.session.id;
        let attached = chat.runtime_id;
        let task = cx.background_executor().spawn(async move {
            match attached {
                Some(runtime_id) => Ok((runtime_id, None)),
                None => {
                    // Re-resolve the runtime: attach, or start one from the
                    // stored session options. Re-hydrating first keeps the
                    // Start options (model, cursor, workspace) fresh.
                    let hydrated = remote::hydrate(&client, session_id)
                        .map_err(|error| error.to_string())?;
                    let runtime = remote::ensure_runtime(&client, &hydrated)
                        .map_err(|error| error.to_string())?;
                    Ok((runtime.runtime_id, Some(runtime.supports_steer)))
                }
            }
        });
        let _entity = cx.entity().downgrade();
        cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, move |this, cx| {
                let mut started_polling = false;
                {
                    let Some(chat) = this.chat.as_mut() else {
                        return;
                    };
                    if chat.session.id != session_id {
                        return;
                    }
                    match result {
                        Ok((runtime_id, supports_steer)) => {
                            let was_unattached = chat.runtime_id.is_none();
                            chat.runtime_id = Some(runtime_id);
                            if let Some(supports_steer) = supports_steer {
                                chat.supports_steer = supports_steer;
                            }
                            if was_unattached {
                                if let Some(client) = this.client.clone() {
                                    chat.events = Some(client.subscribe(session_id, runtime_id));
                                    started_polling = true;
                                }
                            }
                            if let Some(client) = this.client.as_ref() {
                                if let Err(error) = client.notify(
                                    session_id,
                                    runtime_id,
                                    waku_client::Command::Prompt { prompt },
                                ) {
                                    chat.session.status = SessionStatus::Failed;
                                    chat.error = Some(error.to_string());
                                }
                            }
                        }
                        Err(error) => {
                            chat.session.status = SessionStatus::Failed;
                            chat.error = Some(error);
                        }
                    }
                }
                if started_polling {
                    this.start_chat_polling(cx);
                }
                cx.notify();
            });
        })
        .detach();
    }

    pub fn cancel_turn(&mut self, cx: &mut Context<Self>) {
        let Some(chat) = self.chat.as_ref() else {
            return;
        };
        let Some(runtime_id) = chat.runtime_id else {
            return;
        };
        if let Some(client) = self.client.as_ref() {
            let _ = client.notify(chat.session.id, runtime_id, waku_client::Command::Cancel);
        }
        if let Some(chat) = self.chat.as_mut() {
            chat.activity = None;
        }
        cx.notify();
    }

    pub fn respond_permission(&mut self, request_id: String, option_id: String, cx: &mut Context<Self>) {
        let Some(chat) = self.chat.as_ref() else {
            return;
        };
        let Some(runtime_id) = chat.runtime_id else {
            return;
        };
        if let Some(client) = self.client.as_ref() {
            let _ = client.notify(
                chat.session.id,
                runtime_id,
                waku_client::Command::Respond {
                    request_id,
                    option_id,
                },
            );
        }
        if let Some(chat) = self.chat.as_mut() {
            chat.pending_permission = None;
        }
        cx.notify();
    }

    /// Steer the running turn with the composer's current text (when the
    /// provider supports steering).
    pub fn steer_composer(&mut self, cx: &mut Context<Self>) {
        let prompt = self.compose_input.trim().to_owned();
        if prompt.is_empty() {
            return;
        }
        let Some(chat) = self.chat.as_ref() else {
            return;
        };
        let Some(runtime_id) = chat.runtime_id else {
            return;
        };
        let Some(client) = self.client.clone() else {
            return;
        };
        let session_id = chat.session.id;
        self.compose_input.clear();
        let _ = client.notify(
            session_id,
            runtime_id,
            waku_client::Command::Steer { prompt },
        );
        if let Some(chat) = self.chat.as_mut() {
            chat.activity = Some("正在调整方向…".into());
        }
        cx.notify();
    }

    // ── Polling ──

    /// One loop for catalog bumps and connection health; one per open chat
    /// for its driver events. Each wakes the entity on cadence, mirroring the
    /// desktop's event pump cadences.
    fn start_polling(&mut self, cx: &mut Context<Self>) {
        let Some(client) = self.client.clone() else {
            return;
        };
        let task_state = client.subscribe_task_state();
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(1500))
                    .await;
                let should_stop = match this.update(cx, |this, cx| {
                    match this.phase {
                        ConnectionPhase::Connected => {}
                        _ => return true,
                    }
                    // Drain all pending bumps; one reload covers them.
                    let mut bumped = false;
                    while task_state.try_recv().is_ok() {
                        bumped = true;
                    }
                    if bumped && !this.loading_catalog {
                        this.loading_catalog = true;
                        if let Some(client) = this.client.clone() {
                            this.refresh_catalog(&client, cx);
                        }
                    }
                    false
                }) {
                    Ok(stop) => stop,
                    Err(_) => true,
                };
                if should_stop {
                    break;
                }
            }
        })
        .detach();
    }

    fn start_chat_polling(&mut self, cx: &mut Context<Self>) {
        if self.chat.as_ref().is_none_or(|chat| chat.events.is_none()) {
            return;
        }
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(33))
                    .await;
                let alive = this.update(cx, |this, _cx| {
                    this.drain_chat_events();
                    this.chat.as_ref().is_some_and(|chat| chat.events.is_some())
                });
                if alive.is_err() || !alive.unwrap_or(false) {
                    break;
                }
            }
        })
        .detach();
    }

    fn drain_chat_events(&mut self) {
        let Some(events) = self.chat.as_ref().and_then(|chat| chat.events.clone()) else {
            return;
        };
        let Some(chat) = self.chat.as_mut() else {
            return;
        };
        while let Ok(sequenced) = events.try_recv() {
            if chat
                .cursor
                .as_ref()
                .is_some_and(|cursor| {
                    cursor.runtime_id == sequenced.runtime_id
                        && cursor.epoch == sequenced.epoch
                        && cursor.sequence >= sequenced.sequence
                })
            {
                continue;
            }
            chat.cursor = Some(RuntimeEventCursor {
                runtime_id: sequenced.runtime_id,
                epoch: sequenced.epoch,
                sequence: sequenced.sequence,
            });
            let event = match waku_client::event_from_wire(sequenced.event) {
                Ok(event) => event,
                Err(error) => DriverEvent::Error(error.to_string()),
            };
            chat.apply_event(event);
        }
    }

    // ── Navigation ──

    pub fn navigate_to(&mut self, screen: Screen) {
        if self.screen == screen {
            return;
        }
        if self.screen == Screen::Chat {
            // Leaving the chat drops the composer, not the subscription —
            // the daemon keeps streaming and the next open re-hydrates.
        }
        if screen.is_tab_root() {
            self.history.clear();
        } else {
            self.history.push(self.screen);
        }
        self.screen = screen;
    }

    pub fn go_back(&mut self) -> bool {
        if self.screen == Screen::Chat {
            self.history.clear();
            self.screen = Screen::Sessions;
            return true;
        }
        if let Some(previous) = self.history.pop() {
            self.screen = previous;
            true
        } else {
            false
        }
    }

    pub fn can_go_back(&self) -> bool {
        !self.screen.is_tab_root() && !self.history.is_empty()
    }

    /// Drain the platform IME buffer into the focused field. Called at the
    /// top of `render` so text typed since the last frame lands before paint.
    /// Enter in the composer submits the prompt.
    pub fn drain_input(&mut self, cx: &mut Context<Self>) {
        let mut texts: Vec<String> =
            PENDING_IME.with(|buffer| std::mem::take(&mut *buffer.borrow_mut()));
        // Also drain text committed from the Android UI thread (IME paste,
        // QR scan) — it can't use the thread-local callback.
        texts.extend(gpui_mobile::drain_committed_text());
        if texts.is_empty() {
            return;
        }
        for text in texts {
            // A long single commit on the Connect screen is a QR scan result
            // (or a paste) — the whole ticket arrives at once. Fill the
            // ticket field directly instead of relying on active_field, which
            // the scanner's activity switch may have reset.
            if self.screen == crate::screens::Screen::Connect && text.len() > 10 {
                let trimmed = text.trim();
                if !trimmed.is_empty() {
                    self.ticket_input = trimmed.to_owned();
                    self.active_field = crate::screens::connect::FieldTarget::Ticket;
                    self.error = None;
                    continue;
                }
            }
            crate::screens::connect::dispatch_field_input(self, &text);
        }
        // A trailing newline from IME "go" submits instead of leaving a
        // stray blank line in the composer.
        if self.screen == crate::screens::Screen::Chat
            && self.active_field == crate::screens::connect::FieldTarget::Composer
            && self.compose_input.ends_with('\n')
        {
            self.compose_input.pop();
            self.send_composer(cx);
        }
    }
}

// ── Event application ─────────────────────────────────────────────────────

impl ChatSession {
    fn apply_event(&mut self, event: DriverEvent) {
        match event {
            DriverEvent::RuntimeEventCursorAdvanced(_) => {}
            DriverEvent::Connected { .. } => {}
            DriverEvent::AgentPresetSelected(_) => {}
            DriverEvent::AutoTitleUpdated(title) => {
                self.session.auto_title = title;
            }
            DriverEvent::AvailableCommands(_) => {}
            DriverEvent::TurnStarted => {
                self.session.status = SessionStatus::Working;
                self.activity = None;
            }
            DriverEvent::TextDelta(delta) => {
                if let Some(message) = self.streaming_message_mut() {
                    message.content.push_str(&delta);
                } else {
                    let turn_id = self.session.turns.last().map(|turn| turn.id);
                    self.session.messages.push(Message {
                        id: Uuid::new_v4(),
                        turn_id,
                        role: MessageRole::Assistant,
                        display_content: None,
                        content: delta,
                        attachments: Vec::new(),
                        created_at: unix_time(),
                        streaming: true,
                    });
                }
            }
            DriverEvent::ReasoningDelta(_) => {}
            DriverEvent::Activity { title, .. } => {
                self.activity = Some(title);
            }
            DriverEvent::RichActivity(activity) => {
                self.activity = Some(activity.title);
            }
            DriverEvent::BackgroundWork(_) => {}
            DriverEvent::Permission {
                request_id,
                title,
                detail,
                options,
            } => {
                self.pending_permission = Some(PendingPermission {
                    request_id,
                    title,
                    detail,
                    options,
                });
            }
            DriverEvent::UserInputRequested { .. } => {
                self.activity = Some("等待输入确认（请在桌面端处理）".into());
            }
            DriverEvent::ComputerUseUpdated(_) => {}
            DriverEvent::SteerAccepted { .. } => {}
            DriverEvent::SteerRejected { .. } => {}
            DriverEvent::UsageUpdated { .. } => {}
            DriverEvent::PlanUsageUpdated(_) => {}
            DriverEvent::GoalUpdated(_) => {}
            DriverEvent::TurnFinished { success, summary } => {
                for message in &mut self.session.messages {
                    message.streaming = false;
                }
                if let Some(turn) = self.session.turns.last_mut() {
                    turn.status = if success {
                        TurnStatus::Completed
                    } else {
                        TurnStatus::Interrupted
                    };
                }
                self.session.status = SessionStatus::Idle;
                self.activity = None;
                self.pending_permission = None;
                if !success
                    && summary.is_some()
                    && !self
                        .session
                        .messages
                        .iter()
                        .any(|message| message.role == MessageRole::Assistant)
                {
                    self.session
                        .push_message(MessageRole::Assistant, summary.unwrap_or_default());
                }
            }
            DriverEvent::Error(error) => {
                self.error = Some(error);
            }
            DriverEvent::ProcessExited => {
                self.session.status = SessionStatus::Idle;
                self.runtime_id = None;
                self.activity = None;
            }
        }
    }
}
