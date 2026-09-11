declare namespace chrome {
  namespace action {
    const onClicked: Event<(tab: tabs.Tab) => void>;
  }

  namespace alarms {
    interface Alarm {
      readonly name: string;
    }

    const onAlarm: Event<(alarm: Alarm) => void>;
    function create(
      name: string,
      alarmInfo: { readonly periodInMinutes: number },
    ): Promise<void>;
  }

  namespace permissions {
    interface Permissions {
      readonly origins?: readonly string[];
      readonly permissions?: readonly string[];
    }

    function contains(permissions: Permissions): Promise<boolean>;
    function request(permissions: Permissions): Promise<boolean>;
  }

  namespace runtime {
    const id: string;

    interface Manifest {
      readonly version: string;
    }

    interface MessageSender {
      readonly id?: string;
    }

    type MessageListener = (
      message: unknown,
      sender: MessageSender,
      sendResponse: (response: unknown) => void,
    ) => boolean | undefined;

    const onInstalled: Event<(details: { readonly reason: string }) => void>;
    const onStartup: Event<() => void>;
    const onMessage: Event<MessageListener>;
    function getManifest(): Manifest;
    function openOptionsPage(): Promise<void>;
    function sendMessage(message: unknown): Promise<unknown>;
  }

  namespace scripting {
    interface ScriptTarget {
      readonly tabId: number;
    }

    interface InjectionResult<Result> {
      readonly frameId: number;
      readonly result?: Result;
    }

    function executeScript<Result, Args extends readonly unknown[]>(details: {
      readonly target: ScriptTarget;
      readonly func: (...args: Args) => Result;
      readonly args: Args;
    }): Promise<readonly InjectionResult<Awaited<Result>>[]>;
  }

  namespace storage {
    interface StorageArea {
      get(keys: readonly string[]): Promise<Record<string, unknown>>;
      set(items: Record<string, unknown>): Promise<void>;
    }

    interface StorageChange {
      readonly oldValue?: unknown;
      readonly newValue?: unknown;
    }

    const local: StorageArea;
    const onChanged: Event<
      (changes: Record<string, StorageChange>, areaName: string) => void
    >;
  }

  namespace tabs {
    type TabStatus = "unloaded" | "loading" | "complete";

    interface ChangeInfo {
      readonly pendingUrl?: string;
      readonly status?: TabStatus;
      readonly url?: string;
    }

    interface Tab {
      readonly id?: number;
      readonly windowId: number;
      readonly active: boolean;
      readonly status?: TabStatus;
      readonly url?: string;
      readonly pendingUrl?: string;
    }

    const onUpdated: Event<(tabId: number, changeInfo: ChangeInfo) => void>;

    interface QueryInfo {
      readonly active?: boolean;
      readonly windowId?: number;
    }

    interface UpdateProperties {
      readonly active?: boolean;
      readonly url?: string;
    }

    interface ImageDetails {
      readonly format?: "jpeg" | "png";
      readonly quality?: number;
    }

    function get(tabId: number): Promise<Tab>;
    function query(queryInfo: QueryInfo): Promise<readonly Tab[]>;
    function reload(tabId: number): Promise<void>;
    function update(
      tabId: number,
      updateProperties: UpdateProperties,
    ): Promise<Tab | undefined>;
    function captureVisibleTab(
      windowId: number,
      options: ImageDetails,
    ): Promise<string>;
  }

  interface DebuggerTarget {
    readonly tabId?: number;
  }
  interface DebuggerApi {
    attach(target: DebuggerTarget, requiredVersion: string): Promise<void>;
    detach(target: DebuggerTarget): Promise<void>;
    sendCommand(
      target: DebuggerTarget,
      method: string,
      commandParams?: Record<string, unknown>,
    ): Promise<unknown>;
  }

  namespace windows {
    type WindowType = "normal" | "popup" | "panel" | "app" | "devtools";
    type WindowState = "normal" | "minimized" | "maximized" | "fullscreen";

    interface Window {
      readonly id?: number;
      readonly type?: WindowType;
      readonly state?: WindowState;
      readonly focused: boolean;
      readonly incognito: boolean;
      readonly tabs?: readonly tabs.Tab[];
    }

    interface CreateData {
      readonly focused?: boolean;
      readonly height?: number;
      readonly state?: WindowState;
      readonly type?: WindowType;
      readonly url?: string;
      readonly width?: number;
    }

    interface QueryOptions {
      readonly populate?: boolean;
    }

    interface UpdateInfo {
      readonly focused?: boolean;
    }

    function create(createData: CreateData): Promise<Window | undefined>;
    function get(
      windowId: number,
      queryOptions?: QueryOptions,
    ): Promise<Window>;
    function update(windowId: number, updateInfo: UpdateInfo): Promise<Window>;
  }

  interface Event<Listener> {
    addListener(callback: Listener): void;
    removeListener(callback: Listener): void;
  }
}
