import {
  commandFailure,
  commands,
  type SystemRuntimeSnapshot,
  type SystemToolStatus,
} from "@hachimi/contracts";
import { useI18n } from "@hachimi/i18n";
import {
  Badge,
  Button,
  RefreshCw,
  SettingsCard,
  SettingsRow,
  SettingsSection,
  StatusBanner,
} from "@hachimi/ui";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { For, Show, createSignal, onCleanup, onMount } from "solid-js";

function toolLabel(tool: SystemToolStatus, zh: boolean) {
  if (tool.tool === "git") return "Git";
  return zh ? "默认 Shell" : "Default shell";
}

export function SystemToolsSettings() {
  const i18n = useI18n();
  const zh = () => i18n.locale() === "zh-CN";
  const [snapshot, setSnapshot] = createSignal<SystemRuntimeSnapshot>();
  const [busy, setBusy] = createSignal(false);
  const [failure, setFailure] = createSignal<string>();
  let stop: UnlistenFn | undefined;

  async function load(refresh = false) {
    setBusy(true);
    setFailure();
    try {
      setSnapshot(
        refresh ? await commands.refreshSystemRuntime() : await commands.getSystemRuntime(),
      );
    } catch (error) {
      setFailure(commandFailure(error).message);
    } finally {
      setBusy(false);
    }
  }

  onMount(() => {
    void load();
    if (typeof window === "undefined" || !("__TAURI_INTERNALS__" in window)) return;
    void listen<SystemRuntimeSnapshot>("system-runtime-changed", ({ payload }) => {
      setSnapshot(payload);
    }).then((unlisten) => {
      stop = unlisten;
    });
  });
  onCleanup(() => stop?.());

  return (
    <SettingsSection title={zh() ? "系统工具" : "System tools"}>
      <Show when={failure()}>
        {(message) => <StatusBanner tone="danger">{message()}</StatusBanner>}
      </Show>
      <For each={snapshot()?.warnings ?? []}>
        {(warning) => <StatusBanner tone="warning">{warning}</StatusBanner>}
      </For>
      <SettingsCard>
        <For each={snapshot()?.tools ?? []}>
          {(tool) => (
            <SettingsRow
              label={toolLabel(tool, zh())}
              description={[
                tool.executablePath ?? tool.errorCode ?? (zh() ? "未检测到" : "Not detected"),
                tool.version,
                tool.source,
                tool.capabilities.join(", "),
              ]
                .filter(Boolean)
                .join(" · ")}
            >
              <Badge tone={tool.state === "ready" ? "success" : "warning"}>
                {tool.state === "ready"
                  ? zh()
                    ? "已就绪"
                    : "Ready"
                  : tool.state === "degraded"
                    ? zh()
                      ? "能力受限"
                      : "Degraded"
                    : zh()
                      ? "不可用"
                      : "Unavailable"}
              </Badge>
            </SettingsRow>
          )}
        </For>
        <SettingsRow
          label={zh() ? "重新检测" : "Detect again"}
          description={
            zh()
              ? `刷新宿主环境与工具能力；当前 revision ${snapshot()?.revision ?? 0}。`
              : `Refresh the host environment and tool capabilities; current revision ${snapshot()?.revision ?? 0}.`
          }
        >
          <Button
            disabled={busy()}
            data-testid="system-tools-refresh"
            onClick={() => void load(true)}
          >
            <RefreshCw size={14} classList={{ "is-spinning": busy() }} />
            {busy() ? (zh() ? "检测中…" : "Detecting…") : zh() ? "重新检测" : "Detect again"}
          </Button>
        </SettingsRow>
      </SettingsCard>
    </SettingsSection>
  );
}
