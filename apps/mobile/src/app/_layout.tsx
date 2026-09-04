import {
  DarkTheme,
  DefaultTheme,
  Stack,
  ThemeProvider,
  type NativeStackNavigationOptions,
} from "expo-router";
import * as SplashScreen from "expo-splash-screen";
import { StatusBar } from "expo-status-bar";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { useEffect } from "react";
import { StyleSheet, useColorScheme } from "react-native";
import { GestureHandlerRootView } from "react-native-gesture-handler";

import { Colors } from "@/constants/theme";
import { DaemonProvider, useDaemon } from "@/lib/daemon-context";
import { RuntimeProvider } from "@/lib/runtime-context";

/** Deep links and state restores land with the task list beneath them, so a
 * cold-opened task still gets the navigation bar's native back button. */
export const unstable_settings = { anchor: "index" };

void SplashScreen.preventAutoHideAsync();
SplashScreen.setOptions({ duration: 250, fade: true });

const queryClient = new QueryClient({
  defaultOptions: {
    queries: {
      retry: 1,
      refetchOnReconnect: true,
    },
  },
});

/**
 * Native navigation bar with no background of its own: content scrolls
 * beneath it and its Liquid Glass items float. Every screen on the main path
 * shows the bar, the task list included, because that is what lets UIKit hold
 * the bar's buttons in place and crossfade them during an interactive
 * swipe-back; popping to a screen without a bar makes UIKit slide the whole
 * bar out with the page instead.
 */
const floatingHeader = {
  headerShown: true,
  headerStyle: { backgroundColor: "transparent" },
  headerTransparent: true,
} satisfies NativeStackNavigationOptions;

export default function RootLayout() {
  const colorScheme = useColorScheme();
  const colors = Colors[colorScheme === "dark" ? "dark" : "light"];
  const navigationTheme =
    colorScheme === "dark"
      ? {
          ...DarkTheme,
          colors: {
            ...DarkTheme.colors,
            background: colors.background,
            card: colors.background,
          },
        }
      : {
          ...DefaultTheme,
          colors: {
            ...DefaultTheme.colors,
            background: colors.background,
            card: colors.background,
          },
        };
  return (
    <GestureHandlerRootView style={styles.root}>
      <QueryClientProvider client={queryClient}>
        <DaemonProvider>
          <RuntimeProvider>
            <ThemeProvider value={navigationTheme}>
              <AppNavigator />
              <StatusBar style="auto" />
            </ThemeProvider>
          </RuntimeProvider>
        </DaemonProvider>
      </QueryClientProvider>
    </GestureHandlerRootView>
  );
}

function AppNavigator() {
  const { phase } = useDaemon();
  const colorScheme = useColorScheme();
  const theme = Colors[colorScheme === "dark" ? "dark" : "light"];

  useEffect(() => {
    if (phase !== "booting") void SplashScreen.hideAsync();
  }, [phase]);

  return (
    <Stack
      screenOptions={{
        contentStyle: { backgroundColor: theme.background },
        headerBackButtonDisplayMode: "minimal",
        headerShadowVisible: false,
        headerStyle: { backgroundColor: theme.background },
        headerTintColor: theme.text,
      }}
    >
      <Stack.Screen
        name="index"
        options={{ ...floatingHeader, headerTitle: "", title: "Waku" }}
      />
      <Stack.Screen
        name="daemons"
        options={{ headerLargeTitle: true, title: "Daemons" }}
      />
      <Stack.Screen
        name="new-task"
        options={{ ...floatingHeader, title: "New Task" }}
      />
      <Stack.Screen
        name="daemon-editor"
        options={{
          presentation: "pageSheet",
          title: "Add Daemon",
        }}
      />
      <Stack.Screen
        name="session/[id]"
        options={{ ...floatingHeader, headerTitleAlign: "center", title: "Task" }}
      />
    </Stack>
  );
}

const styles = StyleSheet.create({
  root: { flex: 1 },
});
