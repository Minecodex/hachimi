import {
  Button,
  FolderOpen,
  MoreHorizontal,
  PanelLeftClose,
  RefreshCw,
  SlidersHorizontal,
  TerminalSquare,
} from "@hachimi/ui";
import { Show } from "solid-js";

export function WorkbenchToolbar(props: {
  locale: "zh-CN" | "en-US";
  hasProject: boolean;
  hasSession: boolean;
  sessionTitle: string | undefined;
  summaryPinned: boolean;
  bottomPanelOpen: boolean;
  sidebarVisible: boolean;
  terminalReady?: boolean;
  terminalReason?: string | undefined;
  onOpenLocation: () => void;
  onToggleSummary: () => void;
  onToggleBottomPanel: () => void;
  onRefreshSystemRuntime?: () => void;
  onToggleSidebar: () => void;
}) {
  const zh = () => props.locale === "zh-CN";
  return (
    <nav
      class="workbench-toolbar"
      classList={{ "workspace-open": props.sidebarVisible }}
      aria-label={zh() ? "工作台布局" : "Workbench layout"}
    >
      <div class="workbench-conversation-toolbar">
        <Show when={props.sessionTitle}>
          {(title) => (
            <div class="workbench-conversation-title" data-testid="workbench-conversation-title">
              <FolderOpen size={16} />
              <strong>{title()}</strong>
              <MoreHorizontal size={16} aria-hidden="true" />
            </div>
          )}
        </Show>
        <div class="workbench-toolbar-actions">
          <Show when={props.hasSession}>
            <Button
              data-testid="workbench-open-location"
              disabled={!props.hasProject}
              onClick={props.onOpenLocation}
            >
              <FolderOpen size={15} /> {zh() ? "打开位置" : "Open"} ▾
            </Button>
            <Button
              data-testid="workbench-pin-summary"
              class="workbench-toolbar-icon"
              classList={{ active: props.summaryPinned }}
              title={zh() ? "切换置顶摘要" : "Toggle pinned summary"}
              aria-label={zh() ? "切换置顶摘要" : "Toggle pinned summary"}
              onClick={props.onToggleSummary}
            >
              <SlidersHorizontal size={17} />
            </Button>
          </Show>
        </div>
      </div>
      <div class="workbench-workspace-toolbar">
        <Button
          data-testid="workbench-toggle-bottom-panel"
          class="workbench-toolbar-icon"
          classList={{ active: props.bottomPanelOpen }}
          disabled={!props.hasProject || props.terminalReady === false}
          title={
            props.terminalReady === false
              ? `${zh() ? "默认 Shell 不可用" : "Default shell unavailable"}: ${props.terminalReason ?? "system_shell_missing"}`
              : zh()
                ? "切换终端"
                : "Toggle terminal"
          }
          aria-label={zh() ? "切换终端" : "Toggle terminal"}
          onClick={props.onToggleBottomPanel}
        >
          <TerminalSquare size={16} />
        </Button>
        <Show when={props.hasProject && props.terminalReady === false}>
          <Button
            data-testid="workbench-refresh-system-runtime"
            class="workbench-toolbar-icon"
            title={zh() ? "重新检测系统工具" : "Detect system tools again"}
            aria-label={zh() ? "重新检测系统工具" : "Detect system tools again"}
            onClick={props.onRefreshSystemRuntime}
          >
            <RefreshCw size={16} />
          </Button>
        </Show>
        <Button
          data-testid="workbench-toggle-inspector"
          class="workbench-toolbar-icon"
          classList={{ active: props.sidebarVisible }}
          title={zh() ? "切换右侧工作区" : "Toggle right workspace"}
          aria-label={zh() ? "切换右侧工作区" : "Toggle right workspace"}
          onClick={props.onToggleSidebar}
        >
          <PanelLeftClose size={16} />
        </Button>
      </div>
    </nav>
  );
}
