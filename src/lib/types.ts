export type Iso = number;

export interface Conversation {
  id: string;
  title: string;
  createdAt: number;
  updatedAt: number;
  providerId: string | null;
  model: string | null;
  workspaceRoot: string | null;
  systemPrompt: string | null;
  archived: boolean;
  pinned: boolean;
  maxSteps: number;
  toolMode: "direct" | "lazy";
  serversConfigured: boolean;
}

export interface ConversationToolsView {
  serverIds: string[];
  allServers: boolean;
  toolMode: "direct" | "lazy";
}

export interface ContextStatus {
  tokens: number;
  window: number;
  threshold: number;
  keepTokens: number;
  toolCount: number;
  toolMode: string;
}

export interface Message {
  id: string;
  conversationId: string;
  role: "user" | "assistant" | "system" | "tool" | string;
  createdAt: number;
  status: string;
  error: string | null;
}

export interface Part {
  id: string;
  messageId: string;
  idx: number;
  kind: string;
  payloadJson: string;
}

export interface MessageWithParts extends Message {
  parts: Part[];
}

export interface MessagePage {
  messages: MessageWithParts[];
  hasMore: boolean;
}

export interface Attachment {
  id: string;
  name: string;
  mime: string;
  size: number;
  kind: "image" | "text" | "file" | string;
  path: string;
  text?: string | null;
  truncated?: boolean;
}

export interface RevertResult {
  text: string;
  attachments: Attachment[];
}

export interface SearchHit {
  conversationId: string;
  title: string;
  snippet: string;
  updatedAt: number;
}

export interface Provider {
  id: string;
  name: string;
  kind: "openai_compat" | "anthropic" | string;
  baseUrl: string;
  model: string;
  apiKeyRef: string | null;
  contextWindow: number;
  maxOutputTokens: number;
  createdAt: number;
  updatedAt: number;
}

export interface ProviderView extends Provider {
  hasKey: boolean;
  keyHint: string | null;
  models: string[];
}

export type ServerPolicy = "auto" | "ask" | "deny";
export type AuthKind = "none" | "bearer" | "api_key" | "oauth";
export type TransportKind = "streamable_http" | "legacy_http" | "sse";

export interface Server {
  id: string;
  name: string;
  slug: string;
  url: string;
  transport: TransportKind | string;
  authKind: AuthKind | string;
  headersJson: string;
  enabled: boolean;
  policy: ServerPolicy | string;
  protocolVersion: string | null;
  createdAt: number;
  lastStatus: string | null;
  lastError: string | null;
  lastCheckedAt: number | null;
  serverInfoJson: string | null;
  capabilitiesJson: string | null;
  toolsJson: string | null;
  toolsCachedAt: number | null;
  oauthIssuer: string | null;
  oauthClientId: string | null;
  negotiatedAt: number | null;
}

export interface ServerView extends Server {
  hasBearer: boolean;
  hasApiKey: boolean;
  toolCount: number;
}

export interface ServerInfo {
  name: string;
  version: string;
  title?: string | null;
}

export interface ToolInfo {
  name: string;
  title?: string | null;
  description: string;
  inputSchema: Record<string, unknown>;
  outputSchema?: Record<string, unknown> | null;
  annotations?: Record<string, unknown> | null;
}

export interface CallToolResult {
  content: Array<Record<string, unknown>>;
  structuredContent?: unknown;
  isError: boolean;
  resultType?: string;
  inputRequests?: unknown;
  requestState?: unknown;
}

export interface ConnectionReport {
  protocolVersion: string;
  transport: string;
  serverInfo: ServerInfo | null;
  capabilities: Record<string, unknown> | null;
  instructions: string | null;
  tools: ToolInfo[];
}

export type ToolState =
  | "streaming"
  | "queued"
  | "running"
  | "completed"
  | "error"
  | "aborted";

export interface TextPayload {
  text: string;
}

export interface ReasoningPayload {
  text: string;
}

export interface ToolImage {
  name: string;
  mime: string;
  path: string;
  size: number;
}

export interface ToolCallPayload {
  callId: string;
  name: string;
  server: string;
  args: Record<string, unknown>;
  state: ToolState;
  startedAt?: number | null;
  endedAt?: number | null;
  executed: boolean;
  approval?: "pending" | "allowed" | "rejected" | null;
  error?: string | null;
  resultPreview?: string | null;
  fullResultRef?: string | null;
  truncated?: boolean;
  exitCode?: number | null;
  image?: ToolImage | null;
}

export interface ToolResultPayload {
  callId: string;
  name: string;
  server: string;
  ok: boolean;
  content: string;
  structured?: unknown;
  fullResultRef?: string | null;
  truncated?: boolean;
}

export interface ErrorPayload {
  message: string;
}

export interface SummaryPayload {
  text: string;
  coversUntil?: string;
}

export interface UsageEvent {
  type: "usage";
  conversationId: string;
  promptTokens: number;
  completionTokens: number;
  contextWindow: number;
}

export interface ApprovalEvent {
  type: "approval";
  conversationId: string;
  callId: string;
  server: string;
  tool: string;
  args: Record<string, unknown>;
  allowAlways: boolean;
}

export interface QuestionOption {
  label: string;
  description?: string;
}

export interface QuestionPrompt {
  header: string;
  question: string;
  multiple?: boolean;
  options: QuestionOption[];
}

export interface InputRequestEvent {
  type: "input_request";
  conversationId: string;
  callId: string;
  request: {
    inputRequests?: Record<string, InputRequest>;
    requestState?: unknown;
    questions?: QuestionPrompt[];
  };
}

export interface InputRequest {
  method: string;
  params: {
    mode?: "form" | "url";
    message?: string;
    url?: string;
    requestedSchema?: {
      type: "object";
      properties?: Record<string, InputSchemaProperty>;
      required?: string[];
    };
    [key: string]: unknown;
  };
}

export interface InputSchemaProperty {
  type?: string;
  title?: string;
  description?: string;
  enum?: Array<string | number>;
  minimum?: number;
  maximum?: number;
  default?: unknown;
}

export type TurnStatus =
  | "thinking"
  | "streaming"
  | "compacting"
  | "waiting_approval"
  | "running_tool"
  | "idle"
  | "error";

export type CoreEvent =
  | { type: "status"; conversationId: string; status: TurnStatus; message?: string | null }
  | {
      type: "message_start";
      conversationId: string;
      messageId: string;
      role?: string;
    }
  | {
      type: "delta";
      conversationId: string;
      messageId: string;
      partId: string;
      deltaType: "text" | "reasoning";
      text: string;
    }
  | {
      type: "part";
      conversationId: string;
      messageId: string;
      partId: string;
      kind: string;
      payload: Record<string, unknown>;
    }
  | {
      type: "message_end";
      conversationId: string;
      messageId: string;
      status: string;
      error?: string | null;
    }
  | {
      type: "tool";
      conversationId: string;
      callId: string;
      state: string;
      progress?: number | null;
      message?: string | null;
    }
  | ApprovalEvent
  | UsageEvent
  | InputRequestEvent
  | {
      type: "step";
      conversationId: string;
      step: number;
      maxSteps: number;
    }
  | { type: "conversation_updated"; conversationId: string }
  | { type: "resync"; reason: string }
  | { type: "error"; conversationId?: string | null; message: string };

export interface AppInfo {
  name: string;
  version: string;
  dataDir: string;
  workspaceDir: string;
  platform: string;
}

export interface KeyTest {
  ok: boolean;
  models: string[];
  message: string;
}

export interface ExportResult {
  filename: string;
  content: string;
  mime: string;
}

export interface PermissionRule {
  id: string;
  serverId: string | null;
  action: string;
  resource: string;
  effect: string;
  createdAt: number;
}

export interface ProviderInput {
  name: string;
  kind: string;
  baseUrl: string;
  model: string;
  contextWindow: number;
  maxOutputTokens: number;
  apiKey?: string;
  models?: string[];
}

export interface ServerInput {
  name: string;
  url: string;
  authKind: string;
  headersJson: string;
  policy: string;
  bearer?: string;
  apiKey?: string;
  transport?: string;
}

export interface TestServerInput {
  serverId?: string;
  name: string;
  url: string;
  authKind: string;
  headersJson: string;
  bearer?: string;
  apiKey?: string;
  transport?: string;
}
