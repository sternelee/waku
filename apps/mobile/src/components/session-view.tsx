import * as Clipboard from 'expo-clipboard';
import * as Haptics from 'expo-haptics';
import { router, Stack, type NativeStackNavigationOptions } from 'expo-router';
import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import {
  Alert,
  KeyboardAvoidingView,
  Platform,
  StyleSheet,
  Text,
  View,
} from 'react-native';

import { ActivitySheetHost } from '@/components/activity-sheet';
import { AppSymbol } from '@/components/app-symbol';
import { MobileComposer } from '@/components/mobile-composer';
import { RenameDialog } from '@/components/rename-dialog';
import {
  HeaderAction,
  HeaderActionGroup,
  HeaderTitle,
  nativeHeaderButtons,
  navigateBack,
  ScreenHeaderBackdrop,
  useScreenHeaderInset,
  type HeaderActionSpec,
} from '@/components/screen-header';
import { Sheet, SheetRow } from '@/components/sheet';
import {
  TranscriptList,
  type TranscriptDevSample,
  type TranscriptListHandle,
} from '@/components/transcript-list';
import { SessionEmpty } from '@/components/transcript-rows';
import { useSession, useTaskState } from '@/hooks/use-daemon-data';
import { useTheme } from '@/hooks/use-theme';
import { useDaemon } from '@/lib/daemon-context';
import { sessionBusy } from '@/lib/mobile-runtime';
import { useRuntime } from '@/lib/runtime-context';
import { displaySessionTitle } from '@/lib/session-presentation';

export function SessionView({
  sessionId,
  devPrompt,
}: {
  sessionId: string | undefined;
  /** Dev-only: auto-submit this prompt through the composer path once the
   * session loads — lets headless rigs exercise the exact user flow. */
  devPrompt?: string;
}) {
  const theme = useTheme();
  const daemon = useDaemon();
  const runtime = useRuntime();
  const query = useSession(sessionId);
  const session = query.data;
  const [menuOpen, setMenuOpen] = useState(false);
  const [renaming, setRenaming] = useState(false);
  const [underHeader, setUnderHeader] = useState(false);
  const running = Boolean(session && sessionBusy(session));
  const listRef = useRef<TranscriptListHandle>(null);
  const headerInset = useScreenHeaderInset();

  useEffect(() => {
    if (!session || daemon.phase !== 'connected') return;
    void runtime.attachSession(session).catch(() => {});
    // Re-runs when the session starts working (another client may have
    // started the runtime after this screen mounted).
  }, [daemon.phase, runtime.attachSession, session?.id, running]);

  // The header backdrop resets with the session; the list reports the rest.
  useEffect(() => setUnderHeader(false), [session?.id]);

  const probe = useDevProbe(Boolean(devPrompt));

  // Dev-only auto-submit: same path as the composer's send button.
  const devPromptSent = useRef(false);
  useEffect(() => {
    if (!devPrompt || devPromptSent.current) return;
    if (!session || query.isPlaceholderData) {
      probe.setStatus('waiting for session');
      return;
    }
    if (daemon.phase !== 'connected') {
      probe.setStatus(`daemon ${daemon.phase}`);
      return;
    }
    if (sessionBusy(session)) {
      probe.setStatus('session busy');
      return;
    }
    devPromptSent.current = true;
    probe.setStatus('submitting');
    listRef.current?.followNextGrowth();
    runtime.sendPrompt(session, devPrompt)
      .then(() => probe.setStatus('submitted'))
      .catch((cause) => probe.setStatus(`failed ${String(cause).slice(0, 120)}`));
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [daemon.phase, devPrompt, query.isPlaceholderData, session]);

  async function copyLastResponse() {
    const lastAssistant = [...(session?.messages ?? [])]
      .reverse()
      .find((message) => message.role === 'assistant' && message.content.trim());
    if (!lastAssistant) return;
    await Clipboard.setStringAsync(lastAssistant.content);
    await Haptics.notificationAsync(Haptics.NotificationFeedbackType.Success);
  }

  function confirmDelete() {
    if (!session) return;
    Alert.alert(
      `Delete “${displaySessionTitle(session)}”?`,
      'This removes the task and its transcript from the daemon for every device.',
      [
        { text: 'Cancel', style: 'cancel' },
        {
          text: 'Delete',
          style: 'destructive',
          onPress: () => {
            void runtime.deleteSession(session.id)
              .then(() => {
                void Haptics.notificationAsync(Haptics.NotificationFeedbackType.Success);
                navigateBack();
              })
              .catch((cause) => {
                Alert.alert('Couldn’t delete task', cause instanceof Error ? cause.message : String(cause));
              });
          },
        },
      ],
    );
  }

  const projectName = useTaskState().data?.projects
    .find((project) => project.id === session?.project_id)?.name;
  const subtitleParts = [projectName, daemon.activeProfile?.name].filter(Boolean);
  const title = session ? displaySessionTitle(session) : 'Task';
  const subtitle = subtitleParts.length ? subtitleParts.join(' · ') : null;
  const hasSession = Boolean(session);

  // The chrome lives in the native navigation bar, so it stays put while the
  // page slides under a swipe-back. Keyed on the strings, not the session, so
  // streaming updates never touch the bar.
  const headerOptions = useMemo<NativeStackNavigationOptions>(() => {
    const actions: HeaderActionSpec[] = hasSession
      ? [
          {
            icon: { ios: 'square.and.pencil', android: 'edit_square', web: 'edit' },
            label: 'New task',
            onPress: () => router.push('/new-task'),
          },
          {
            icon: { ios: 'ellipsis', android: 'more_horiz', web: 'more_horiz' },
            label: 'Task options',
            onPress: () => setMenuOpen(true),
          },
        ]
      : [];
    return {
      headerTitle: () => <HeaderTitle subtitle={subtitle} title={title} />,
      headerRight: actions.length
        ? () => (
            <HeaderActionGroup>
              {actions.map((action) => <HeaderAction key={action.label} {...action} />)}
            </HeaderActionGroup>
          )
        : undefined,
      unstable_headerRightItems: actions.length ? () => nativeHeaderButtons(actions) : undefined,
    };
  }, [hasSession, subtitle, title]);

  return (
    <KeyboardAvoidingView
      behavior={Platform.OS === 'ios' ? 'padding' : undefined}
      style={[styles.screen, { backgroundColor: theme.background }]}>
      <Stack.Screen options={headerOptions} />
      <View style={styles.body}>
        {session ? (
          <ActivitySheetHost key={session.id} session={session}>
            <TranscriptList
              headerInset={headerInset}
              hydrated={!query.isPlaceholderData}
              offline={daemon.phase === 'error'}
              ref={listRef}
              running={running}
              session={session}
              onDevSample={devPrompt ? probe.sample : undefined}
              onUnderHeaderChange={setUnderHeader}
            />
          </ActivitySheetHost>
        ) : (
          <View style={styles.placeholder}>
            <SessionEmpty error={query.error} loading={query.isPending} missing={query.data === null} />
          </View>
        )}
        {Boolean(devPrompt && probe.text) && (
          <View pointerEvents="none" style={[styles.devBadge, { top: headerInset + 8 }]}>
            <Text style={styles.devBadgeText}>{probe.text}</Text>
          </View>
        )}
      </View>
      <ScreenHeaderBackdrop visible={underHeader} />
      {session && (
        <MobileComposer
          session={session}
          onSubmitted={() => listRef.current?.followNextGrowth()}
        />
      )}

      <Sheet onDismiss={() => setMenuOpen(false)} visible={menuOpen}>
        <SheetRow
          label="Rename task"
          leading={<AppSymbol name={{ ios: 'pencil', android: 'edit', web: 'edit' }} size={16} tintColor={theme.textSecondary} />}
          onPress={() => {
            setMenuOpen(false);
            setRenaming(true);
          }}
        />
        <SheetRow
          label="Copy last response"
          leading={<AppSymbol name={{ ios: 'doc.on.doc', android: 'content_copy', web: 'content_copy' }} size={16} tintColor={theme.textSecondary} />}
          onPress={() => {
            setMenuOpen(false);
            void copyLastResponse();
          }}
        />
        <SheetRow
          label="Reload transcript"
          leading={<AppSymbol name={{ ios: 'arrow.clockwise', android: 'refresh', web: 'refresh' }} size={16} tintColor={theme.textSecondary} />}
          onPress={() => {
            setMenuOpen(false);
            void query.refetch();
          }}
        />
        <SheetRow
          destructive
          label="Delete task"
          leading={<AppSymbol name={{ ios: 'trash', android: 'delete', web: 'delete' }} size={16} tintColor={theme.danger} />}
          onPress={() => {
            setMenuOpen(false);
            confirmDelete();
          }}
        />
      </Sheet>
      {session && (
        <RenameDialog
          initialValue={displaySessionTitle(session)}
          onDismiss={() => setRenaming(false)}
          onSubmit={(title) => runtime.renameSession(session.id, title)}
          visible={renaming}
        />
      )}
    </KeyboardAvoidingView>
  );
}

/**
 * Dev-only motion probe. Screen recorders capture ~8 real fps and cannot
 * tell a seated stream from a bouncing one; the scroll events can. While a
 * stream is followed the native offset must read 0 — `drift` is the largest
 * untouched offset seen in the last second, and `grow` counts content-size
 * commits, so "drift 0" across a fast stream is the pass condition.
 */
function useDevProbe(enabled: boolean) {
  const [status, setStatus] = useState('');
  const [text, setText] = useState('');
  const samples = useRef<TranscriptDevSample[]>([]);
  const rates = useRef<Array<{ at: number; count: number; flips: number }>>([]);
  const growths = useRef(0);
  const lastHeight = useRef(0);

  const sample = useCallback((next: TranscriptDevSample) => {
    if (next.contentHeight !== lastHeight.current) {
      if (lastHeight.current > 0) growths.current += 1;
      lastHeight.current = next.contentHeight;
    }
    samples.current.push(next);
  }, []);

  useEffect(() => {
    if (!enabled) return;
    const timer = setInterval(() => {
      const now = Date.now();
      const recent = samples.current.filter((item) => now - item.at < 1_000);
      samples.current = recent;
      let drift = 0;
      for (const item of recent) {
        if (!item.touching) drift = Math.max(drift, item.offset);
      }
      const latest = recent.at(-1);
      const scrolls = recent.filter((item) => item.source === 'scroll');
      // Direction reversals between consecutive scroll samples: an animation
      // is monotonic, a compensation fight alternates.
      let flips = 0;
      for (let ix = 2; ix < scrolls.length; ix += 1) {
        const a = scrolls[ix - 1]!.offset - scrolls[ix - 2]!.offset;
        const b = scrolls[ix]!.offset - scrolls[ix - 1]!.offset;
        if (a * b < 0) flips += 1;
      }
      // Peaks over the last five seconds survive the snapshot latency of an
      // external reader.
      rates.current = [
        ...rates.current.filter((item) => now - item.at < 5_000),
        { at: now, count: scrolls.length, flips },
      ];
      const peak = Math.max(...rates.current.map((item) => item.count));
      const peakFlips = Math.max(...rates.current.map((item) => item.flips));
      setText(
        `dev: ${status} · scr ${scrolls.length}/s (peak ${peak}, flips ${peakFlips}) · size ${recent.length - scrolls.length}/s · off ${latest ? latest.offset.toFixed(0) : '–'} · drift ${drift.toFixed(0)} · grow ${growths.current} · touch ${latest ? (latest.touching ? 1 : 0) : '–'}`,
      );
    }, 500);
    return () => clearInterval(timer);
  }, [enabled, status]);

  return { sample, setStatus, text: enabled ? text : '' };
}

const styles = StyleSheet.create({
  screen: { flex: 1 },
  body: { flex: 1 },
  placeholder: { alignItems: 'center', flex: 1, justifyContent: 'center', paddingHorizontal: 32 },
  devBadge: {
    backgroundColor: 'rgba(0,0,0,0.75)',
    borderRadius: 6,
    left: 12,
    padding: 6,
    position: 'absolute',
  },
  devBadgeText: { color: '#fff', fontSize: 11 },
});
