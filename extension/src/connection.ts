import {
  type CommandEnvelope,
  HEARTBEAT_INTERVAL_MS,
  makeHeartbeatEnvelope,
  PROTOCOL_VERSION,
  parseServerMessage,
  type ResultData,
} from "./protocol";
import { BrowserExecutionError, BrowserExecutor } from "./browser-executor";
import { DRIVER_CAPABILITIES } from "./drivers";
import {
  type CommandLedgerEntry,
  type ConnectionSettings,
  type ConnectionStatus,
  DEFAULT_SETTINGS,
  discoverServerUrl,
  findCommandLedgerEntry,
  makeWebSocketUrl,
  parseConnectionSettings,
  readSettings,
  readStatus,
  recordCommand,
  writeSettings,
  writeStatus,
} from "./state";

const RECONNECT_ALARM = "pluk-browser-reconnect";
const PAUSED_MESSAGE = "Paused. Turn on Stay connected to reconnect.";
const MAX_RECONNECT_DELAY_MS = 30_000;
const INITIAL_RECONNECT_DELAY_MS = 1_000;
const ARTIFACT_UPLOAD_TIMEOUT_MS = 10_000;
const MAX_RESPONSE_BYTES = 64 * 1024;

interface BridgeSettingsView {
  readonly serverUrl: string;
  readonly enabled: boolean;
  readonly hasToken: boolean;
}

interface BridgeSuccessResponse {
  readonly ok: true;
  readonly settings?: BridgeSettingsView;
  readonly status?: ConnectionStatus;
}

interface BridgeErrorResponse {
  readonly ok: false;
  readonly error: string;
}

type BridgeResponse = BridgeSuccessResponse | BridgeErrorResponse;

export class BrowserBridge {
  private readonly browserExecutor = new BrowserExecutor();
  private settings: ConnectionSettings = DEFAULT_SETTINGS;
  private socket: WebSocket | null = null;
  private socketGeneration = 0;
  private serverReady = false;
  private helloSent = false;
  private heartbeatTimer: ReturnType<typeof setInterval> | null = null;
  private reconnectTimer: ReturnType<typeof setTimeout> | null = null;
  private reconnectDelayMs = INITIAL_RECONNECT_DELAY_MS;
  private commandChain = Promise.resolve();
  private initializePromise: Promise<void> | null = null;

  initialize(): Promise<void> {
    if (this.initializePromise === null) {
      this.initializePromise = this.initializeOnce();
    }
    return this.initializePromise;
  }

  async handleMessage(message: unknown): Promise<BridgeResponse> {
    await this.initialize();
    if (!isRecord(message) || typeof message.type !== "string") {
      return { ok: false, error: "This request is not supported." };
    }
    if (message.type === "get_status") {
      return await this.statusResponse();
    }
    if (message.type === "save_settings") {
      return this.saveSettings(message.settings);
    }
    return { ok: false, error: "This request is not supported." };
  }

  handleReconnectAlarm(name: string): void {
    if (name !== RECONNECT_ALARM) {
      return;
    }
    void this.initialize().then(() => {
      if (
        this.settings.enabled &&
        (this.socket === null || this.socket.readyState === WebSocket.CLOSED)
      ) {
        void this.findPlukAndConnect();
      }
    });
  }

  private async initializeOnce(): Promise<void> {
    try {
      await chrome.alarms.create(RECONNECT_ALARM, {
        periodInMinutes: 1,
      });
      this.settings = await readSettings();
      if (!this.settings.enabled) {
        await this.setStatus("disabled", PAUSED_MESSAGE);
        return;
      }
      if (this.settings.token === "") {
        this.settings = DEFAULT_SETTINGS;
        await this.setStatus("not_configured", "Paste your Pluk ID to connect.");
        return;
      }
      await this.setStatus("connecting", "Connecting to Pluk.");
      this.connect();
    } catch {
      await this.setStatus(
        "error",
        "Could not read your saved setup. Save it again.",
      );
    }
  }

  private async statusResponse(): Promise<BridgeSuccessResponse> {
    const status = await readStatus();
    return {
      ok: true,
      settings: {
        serverUrl: this.settings.serverUrl,
        enabled: this.settings.enabled,
        hasToken: this.settings.token !== "",
      },
      status,
    };
  }

  private async saveSettings(value: unknown): Promise<BridgeResponse> {
    const settings = parseConnectionSettings(value);
    if (settings === null) {
      return {
        ok: false,
        error: "Paste your Pluk ID, exactly as Pluk shows it.",
      };
    }
    try {
      await writeSettings(settings);
    } catch {
      return { ok: false, error: "Could not save this. Try again." };
    }
    this.settings = settings;
    this.reconnectDelayMs = INITIAL_RECONNECT_DELAY_MS;
    this.clearReconnectTimer();
    this.closeSocket();
    if (!settings.enabled) {
      await this.setStatus("disabled", PAUSED_MESSAGE);
      return { ok: true, settings: settingsView(settings) };
    }
    await this.setStatus("connecting", "Connecting to Pluk.");
    await this.findPlukAndConnect();
    return { ok: true, settings: settingsView(this.settings) };
  }

  // Pluk answers on 4242 unless `PORT` moves it. Probe the loopback ports,
  // keep whichever one answers, then dial that.
  private async findPlukAndConnect(): Promise<void> {
    try {
      const discovered = await discoverServerUrl(this.settings.serverUrl);
      if (discovered !== null && discovered !== this.settings.serverUrl) {
        this.settings = { ...this.settings, serverUrl: discovered };
        await writeSettings(this.settings);
      }
    } catch {
      await this.setStatus(
        "error",
        "Could not save where Pluk is. Try saving again.",
      );
    }
    this.connect();
  }

  private connect(): void {
    if (!this.settings.enabled || this.settings.token === "") {
      return;
    }
    if (
      this.socket !== null &&
      (this.socket.readyState === WebSocket.OPEN ||
        this.socket.readyState === WebSocket.CONNECTING)
    ) {
      return;
    }

    const generation = ++this.socketGeneration;
    let socket: WebSocket;
    try {
      socket = new WebSocket(
        makeWebSocketUrl(this.settings.serverUrl, this.settings.token),
      );
    } catch {
      this.scheduleReconnect();
      void this.setStatus(
        "error",
        "Pluk is not running. Start it and this will reconnect.",
      );
      return;
    }
    this.socket = socket;
    this.serverReady = false;
    this.helloSent = false;
    socket.addEventListener("open", () => {
      this.handleSocketOpen(socket, generation);
    });
    socket.addEventListener("message", (event) => {
      this.handleSocketMessage(socket, generation, event.data);
    });
    socket.addEventListener("close", () => {
      this.handleSocketClose(socket, generation);
    });
    socket.addEventListener("error", () => {
      if (this.isCurrentSocket(socket, generation)) {
        void this.setStatus(
          "error",
          "Pluk is not running. Start it and this will reconnect.",
        );
      }
    });
  }

  private handleSocketOpen(socket: WebSocket, generation: number): void {
    if (!this.isCurrentSocket(socket, generation)) {
      socket.close();
      return;
    }
    this.reconnectDelayMs = INITIAL_RECONNECT_DELAY_MS;
    this.sendHello(socket);
    this.startHeartbeat(socket, generation);
    void this.setStatus("connecting", "Waiting for Pluk.");
  }

  private handleSocketMessage(
    socket: WebSocket,
    generation: number,
    data: unknown,
  ): void {
    if (!this.isCurrentSocket(socket, generation)) {
      return;
    }
    if (typeof data !== "string") {
      this.closeSocket(socket, true);
      return;
    }
    if (new TextEncoder().encode(data).byteLength > MAX_RESPONSE_BYTES) {
      this.closeSocket(socket, true);
      return;
    }

    let value: unknown;
    try {
      value = JSON.parse(data);
    } catch {
      this.closeSocket(socket, true);
      return;
    }
    const parsed = parseServerMessage(value);
    if (!parsed.ok) {
      this.closeSocket(socket, true);
      return;
    }
    if (parsed.value.type === "ready") {
      if (!this.helloSent || parsed.value.expiresAt <= Date.now()) {
        this.closeSocket(socket, true);
        return;
      }
      this.serverReady = true;
      void this.setStatus("connected", "Connected to Pluk.");
      return;
    }
    if (parsed.value.type === "heartbeat") {
      if (parsed.value.expiresAt <= Date.now()) {
        this.closeSocket(socket, true);
        return;
      }
      this.sendHeartbeat(socket);
      return;
    }
    if (parsed.value.type === "heartbeat_ack") {
      return;
    }
    if (!this.serverReady) {
      this.closeSocket(socket, true);
      return;
    }
    this.enqueueCommand(parsed.value, socket, generation);
  }

  private handleSocketClose(socket: WebSocket, generation: number): void {
    if (!this.isCurrentSocket(socket, generation)) {
      return;
    }
    this.socket = null;
    this.serverReady = false;
    this.helloSent = false;
    this.stopHeartbeat();
    if (this.settings.enabled) {
      void this.setStatus(
        "error",
        "Pluk is not running. Start it and this will reconnect.",
      );
      this.scheduleReconnect();
    } else {
      void this.setStatus("disabled", PAUSED_MESSAGE);
    }
  }

  private enqueueCommand(
    command: CommandEnvelope,
    socket: WebSocket,
    generation: number,
  ): void {
    this.commandChain = this.commandChain.then(
      () => this.processCommand(command, socket, generation),
      () => this.processCommand(command, socket, generation),
    );
  }

  private async processCommand(
    command: CommandEnvelope,
    socket: WebSocket,
    generation: number,
  ): Promise<void> {
    let existing: CommandLedgerEntry | null;
    try {
      existing = await findCommandLedgerEntry(command.commandId);
    } catch {
      await this.sendFailedResult(
        command,
        socket,
        generation,
        "state_unavailable",
        "Could not safely record this task, so it was not run. Try again.",
      );
      return;
    }
    if (existing !== null) {
      const error =
        existing.state === "inflight"
          ? {
              code: "uncertain_execution",
              message:
                "An earlier attempt stopped partway. This was not run again; check the page before retrying.",
            }
          : {
              code: "duplicate_command",
              message:
                "This command was already handled and was not run again.",
            };
      await this.sendFailedResult(
        command,
        socket,
        generation,
        error.code,
        error.message,
      );
      return;
    }

    if (Date.now() >= command.expiresAt) {
      await this.recordCompleted(command);
      return;
    }

    try {
      await recordCommand(command.commandId, command.action, "inflight");
    } catch {
      await this.sendFailedResult(
        command,
        socket,
        generation,
        "state_unavailable",
        "Could not safely record this task, so it was not run. Try again.",
      );
      return;
    }

    let data: ResultData | null = null;
    let error: { readonly code: string; readonly message: string } | null =
      null;
    try {
      data = await this.browserExecutor.run(command, {
        upload: (jobId, kind, contentType, body) =>
          this.uploadArtifact(jobId, kind, contentType, body),
      });
    } catch (caught) {
      error = toProtocolError(caught);
    }

    try {
      await recordCommand(command.commandId, command.action, "completed");
    } catch {
      data = null;
      error = {
        code: "uncertain_side_effect",
        message:
          "Could not safely record the result. Nothing was retried; check the page before running this again.",
      };
    }
    if (Date.now() >= command.expiresAt) {
      return;
    }
    if (error !== null) {
      await this.sendFailedResult(
        command,
        socket,
        generation,
        error.code,
        error.message,
      );
      return;
    }
    if (data === null) {
      await this.sendFailedResult(
        command,
        socket,
        generation,
        "missing_result",
        "The page returned no result. Try again.",
      );
      return;
    }
    await this.sendResult(command, socket, generation, data);
  }

  private async recordCompleted(command: CommandEnvelope): Promise<void> {
    try {
      await recordCommand(command.commandId, command.action, "completed");
    } catch {
      return;
    }
  }

  private async sendFailedResult(
    command: CommandEnvelope,
    socket: WebSocket,
    generation: number,
    code: string,
    message: string,
  ): Promise<void> {
    await this.sendResult(command, socket, generation, null, { code, message });
  }

  private async sendResult(
    command: CommandEnvelope,
    socket: WebSocket,
    generation: number,
    data: ResultData | null,
    error: { readonly code: string; readonly message: string } | null = null,
  ): Promise<void> {
    if (!this.isCurrentSocket(socket, generation)) {
      return;
    }
    const envelope = {
      version: PROTOCOL_VERSION,
      type: "result" as const,
      jobId: command.jobId,
      commandId: command.commandId,
      issuedAt: Date.now(),
      expiresAt: command.expiresAt,
      outcome: error === null ? ("succeeded" as const) : ("failed" as const),
      ...(error === null ? { data } : { error }),
    };
    const encoded = JSON.stringify(envelope);
    if (new TextEncoder().encode(encoded).byteLength > MAX_RESPONSE_BYTES) {
      await this.sendFailedResult(
        command,
        socket,
        generation,
        "result_too_large",
        "The page snapshot exceeded the message limit. Try a smaller page.",
      );
      return;
    }
    try {
      socket.send(encoded);
    } catch {
      this.closeSocket(socket, true);
    }
  }

  private sendHello(socket: WebSocket): void {
    if (this.helloSent || socket.readyState !== WebSocket.OPEN) {
      return;
    }
    const now = Date.now();
    const hello = {
      version: PROTOCOL_VERSION,
      type: "hello" as const,
      extensionVersion: chrome.runtime.getManifest().version,
      capabilities: DRIVER_CAPABILITIES,
      issuedAt: now,
      expiresAt: now + HEARTBEAT_INTERVAL_MS,
    };
    try {
      socket.send(JSON.stringify(hello));
      this.helloSent = true;
    } catch {
      this.closeSocket(socket, true);
    }
  }

  private startHeartbeat(socket: WebSocket, generation: number): void {
    this.stopHeartbeat();
    this.heartbeatTimer = setInterval(() => {
      if (!this.isCurrentSocket(socket, generation)) {
        this.stopHeartbeat();
        return;
      }
      this.sendHeartbeat(socket);
    }, HEARTBEAT_INTERVAL_MS);
  }

  private sendHeartbeat(socket: WebSocket): void {
    if (socket.readyState !== WebSocket.OPEN) {
      return;
    }
    try {
      socket.send(JSON.stringify(makeHeartbeatEnvelope(Date.now())));
    } catch {
      this.closeSocket(socket, true);
    }
  }

  private scheduleReconnect(): void {
    if (this.reconnectTimer !== null || !this.settings.enabled) {
      return;
    }
    const delay = this.reconnectDelayMs;
    this.reconnectDelayMs = Math.min(
      MAX_RECONNECT_DELAY_MS,
      this.reconnectDelayMs * 2,
    );
    this.reconnectTimer = setTimeout(() => {
      this.reconnectTimer = null;
      void this.findPlukAndConnect();
    }, delay);
  }

  private clearReconnectTimer(): void {
    if (this.reconnectTimer !== null) {
      clearTimeout(this.reconnectTimer);
      this.reconnectTimer = null;
    }
  }

  private closeSocket(socket = this.socket, reconnect = false): void {
    if (socket === null) {
      return;
    }
    const isCurrent = this.socket === socket;
    if (isCurrent) {
      this.socket = null;
      this.serverReady = false;
      this.helloSent = false;
      this.stopHeartbeat();
      if (reconnect && this.settings.enabled) {
        void this.setStatus(
          "error",
          "Lost the connection to Pluk. Reconnecting.",
        );
        this.scheduleReconnect();
      }
    }
    if (
      socket.readyState === WebSocket.OPEN ||
      socket.readyState === WebSocket.CONNECTING
    ) {
      socket.close();
    }
  }

  private stopHeartbeat(): void {
    if (this.heartbeatTimer !== null) {
      clearInterval(this.heartbeatTimer);
      this.heartbeatTimer = null;
    }
  }

  private isCurrentSocket(socket: WebSocket, generation: number): boolean {
    return this.socket === socket && this.socketGeneration === generation;
  }

  private async setStatus(
    state: ConnectionStatus["state"],
    message: string,
  ): Promise<void> {
    await writeStatus({ state, message, updatedAt: Date.now() });
  }

  private async uploadArtifact(
    jobId: string,
    kind: "screenshot" | "extract",
    contentType: string,
    body: Uint8Array,
  ): Promise<string> {
    if (!this.settings.enabled || this.settings.token === "") {
      throw new BrowserExecutionError(
        "artifact_upload_failed",
        "Not connected to Pluk, so the snapshot was not saved.",
      );
    }
    const controller = new AbortController();
    const timeout = setTimeout(
      () => controller.abort(),
      ARTIFACT_UPLOAD_TIMEOUT_MS,
    );
    let response: Response;
    try {
      const uploadBody = new Uint8Array(new ArrayBuffer(body.byteLength));
      uploadBody.set(body);
      response = await fetch(
        `${this.settings.serverUrl}/wande/jobs/${encodeURIComponent(jobId)}/artifacts`,
        {
          method: "POST",
          headers: {
            Authorization: `Bearer ${this.settings.token}`,
            "Content-Type": contentType,
            "X-Wande-Artifact-Kind": kind,
          },
          body: uploadBody,
          credentials: "omit",
          signal: controller.signal,
        },
      );
    } catch {
      throw new BrowserExecutionError(
        "artifact_upload_failed",
        "Pluk did not accept the snapshot. Start it and try again.",
      );
    } finally {
      clearTimeout(timeout);
    }
    if (!response.ok) {
      throw new BrowserExecutionError(
        "artifact_upload_failed",
        "Pluk rejected the snapshot. Check your Pluk ID and try again.",
      );
    }
    const length = response.headers.get("content-length");
    if (length !== null && Number(length) > MAX_RESPONSE_BYTES) {
      throw new BrowserExecutionError(
        "artifact_upload_failed",
        "Pluk returned an oversized response.",
      );
    }
    const text = await response.text();
    if (new TextEncoder().encode(text).byteLength > MAX_RESPONSE_BYTES) {
      throw new BrowserExecutionError(
        "artifact_upload_failed",
        "Pluk returned an oversized response.",
      );
    }
    let value: unknown;
    try {
      value = JSON.parse(text);
    } catch {
      throw new BrowserExecutionError(
        "artifact_upload_failed",
        "Pluk returned an invalid snapshot response.",
      );
    }
    const id = readArtifactId(value);
    if (id === null) {
      throw new BrowserExecutionError(
        "artifact_upload_failed",
        "Pluk returned an invalid snapshot response.",
      );
    }
    return id;
  }
}

function settingsView(settings: ConnectionSettings): BridgeSettingsView {
  return {
    serverUrl: settings.serverUrl,
    enabled: settings.enabled,
    hasToken: settings.token !== "",
  };
}

function toProtocolError(error: unknown): {
  readonly code: string;
  readonly message: string;
} {
  if (error instanceof BrowserExecutionError) {
    return { code: error.code, message: error.message };
  }
  return {
    code: "browser_error",
    message: "This could not be completed. Try again.",
  };
}

function readArtifactId(value: unknown): string | null {
  if (!isRecord(value) || !isRecord(value.artifact)) {
    return null;
  }
  const id = value.artifact.id;
  return typeof id === "string" && /^[A-Za-z0-9-]{1,128}$/u.test(id)
    ? id
    : null;
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}
