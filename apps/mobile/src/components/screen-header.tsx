import { BlurView } from "expo-blur";
import { router, type NativeStackHeaderItem } from "expo-router";
import { useHeaderHeight } from "expo-router/build/react-navigation/elements";
import type { ReactNode } from "react";
import {
  Pressable,
  StyleSheet,
  Text,
  View,
  useColorScheme,
  useWindowDimensions,
} from "react-native";

import { AppSymbol } from "./app-symbol";
import { GlassSurface } from "./glass-surface";
import { Radius } from "@/constants/theme";
import { useTheme } from "@/hooks/use-theme";

/** Pop when there is history; otherwise land on the task list. A screen
 * opened cold (deep link, state restore) is the stack's only entry, and a
 * bare router.back() there throws GO_BACK unhandled. */
export function navigateBack() {
  if (router.canGoBack()) router.back();
  else router.replace("/");
}

/**
 * Bottom edge of the transparent native navigation bar, measured from the top
 * of the screen. Content that runs under the bar insets by this. Rounded so
 * the native height report, a fraction off the JS default, does not re-layout
 * the transcript once it lands.
 */
export function useScreenHeaderInset() {
  return Math.round(useHeaderHeight());
}

/**
 * Translucent chrome backdrop shown once content has scrolled under the
 * navigation bar, like a native bar's scroll-edge treatment. It lives in the
 * screen content, so it travels with the page during a swipe-back while the
 * bar's buttons and title stay put in the native navigation bar above it.
 */
export function ScreenHeaderBackdrop({ visible }: { visible: boolean }) {
  const theme = useTheme();
  const colorScheme = useColorScheme();
  const height = useScreenHeaderInset();
  if (!visible) return null;
  return (
    <View
      pointerEvents="none"
      style={[
        styles.backdrop,
        {
          height,
          borderBottomColor: theme.borderStrong,
          backgroundColor: colorScheme === "dark" ? "#333333e3" : "#ffffffd6",
        },
      ]}
    >
      <BlurView intensity={6} style={StyleSheet.absoluteFill} />
    </View>
  );
}

/** Width kept clear on each side of the title for the bar's own items: the
 * round back button on the left, a two-action glass capsule on the right.
 * The title view is laid out by Yoga from its content, so it has to bound
 * itself; UIKit only centers it. */
const TitleSideReserve = 120;

/**
 * Navigation bar title with an optional "project · daemon" subtitle. Rendered
 * as the bar's native title view, so UIKit owns it during transitions.
 */
export function HeaderTitle({
  title,
  subtitle,
}: {
  title: string;
  subtitle?: string | null;
}) {
  const theme = useTheme();
  const { width } = useWindowDimensions();
  const maxWidth = Math.max(TitleSideReserve, width - TitleSideReserve * 2);
  return (
    <View style={[styles.titles, { maxWidth }]}>
      <Text numberOfLines={1} style={[styles.title, { color: theme.text }]}>
        {title}
      </Text>
      {subtitle ? (
        <Text
          numberOfLines={1}
          style={[styles.subtitle, { color: theme.textTertiary }]}
        >
          {subtitle}
        </Text>
      ) : null}
    </View>
  );
}

export type HeaderActionSpec = {
  icon: Parameters<typeof AppSymbol>[0]["name"];
  label: string;
  onPress: () => void;
};

/** Native bar button items for iOS: SF Symbol glyphs in the system's shared
 * Liquid Glass capsule, grouped and transitioned by UIKit. */
export function nativeHeaderButtons(
  actions: HeaderActionSpec[],
): NativeStackHeaderItem[] {
  return actions.map((action) => {
    const symbol =
      typeof action.icon === "string" ? action.icon : action.icon.ios;
    return {
      type: "button",
      label: action.label,
      accessibilityLabel: action.label,
      ...(symbol ? { icon: { type: "sfSymbol", name: symbol } } : {}),
      onPress: action.onPress,
    };
  });
}

/** Pill grouping trailing header actions for platforms without native bar
 * button items, like the reference's [compose | …]. */
export function HeaderActionGroup({ children }: { children: ReactNode }) {
  return (
    <GlassSurface interactive style={styles.actionGroup}>
      {children}
    </GlassSurface>
  );
}

export function HeaderAction({ icon, label, onPress }: HeaderActionSpec) {
  const theme = useTheme();
  return (
    <Pressable
      accessibilityLabel={label}
      accessibilityRole="button"
      hitSlop={4}
      onPress={onPress}
      style={({ pressed }) => [styles.action, { opacity: pressed ? 0.5 : 1 }]}
    >
      <AppSymbol name={icon} size={17} tintColor={theme.text} />
    </Pressable>
  );
}

const styles = StyleSheet.create({
  backdrop: {
    borderBottomWidth: StyleSheet.hairlineWidth,
    left: 0,
    overflow: "hidden",
    position: "absolute",
    right: 0,
    top: 0,
    zIndex: 20,
  },
  titles: {
    alignItems: "center",
    justifyContent: "center",
    minWidth: 0,
  },
  title: {
    fontSize: 17,
    fontWeight: "700",
    letterSpacing: -0.3,
    textAlign: "center",
  },
  subtitle: { fontSize: 12.5, marginTop: 1, textAlign: "center" },
  actionGroup: {
    alignItems: "center",
    borderRadius: Radius.pill,
    flexDirection: "row",
    height: 44,
    paddingHorizontal: 4,
  },
  action: {
    alignItems: "center",
    height: 44,
    justifyContent: "center",
    width: 42,
  },
});
