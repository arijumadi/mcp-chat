import type {
  ConnectionReport,
  Conversation,
  CoreEvent,
  ExportResult,
  MessageWithParts,
  Part,
  ProviderView,
  ServerView,
  ToolInfo,
} from "./types";
import type { MockBridge } from "./ipc";

const now = () => Date.now();
const uid = () => Math.random().toString(36).slice(2, 10);

const TOOL: ToolInfo = {
  name: "echo",
  description: "Echo text back",
  inputSchema: {
    type: "object",
    properties: { text: { type: "string" } },
    required: ["text"],
  },
};

export function installMock(): void {
  if (!new URLSearchParams(location.search).has("mock")) return;
  if (window.__MCP_CHAT_MOCK__) return;

  const empty = new URLSearchParams(location.search).get("mock") === "empty";
  const conversations: Conversation[] = [];
  const messages = new Map<string, MessageWithParts[]>();
  const subscribers = new Set<(event: CoreEvent) => void>();
  const generating = new Set<string>();
  const autoApprove = new Set<string>();
  const settingsStore: Record<string, string> = {
    theme: localStorage.getItem("theme") ?? "system",
    "compaction.keep_tokens": "15000",
    "compaction.buffer": "0",
  };
  let providers: ProviderView[] = [
    {
      id: "provider-mock",
      name: "Mock provider",
      kind: "openai_compat",
      baseUrl: "http://127.0.0.1:8788/v1",
      model: "mock-text",
      apiKeyRef: "provider-mock",
      contextWindow: 128000,
      maxOutputTokens: 8192,
      createdAt: now(),
      updatedAt: now(),
      hasKey: true,
      keyHint: "tes••••key",
      models: ["mock-text", "mock-tool", "mock-parallel"],
    },
  ];
  const servers: ServerView[] = [
    {
      id: "server-mock",
      name: "Mock MCP",
      slug: "mock",
      url: "http://127.0.0.1:8787/mcp",
      transport: "streamable_http",
      authKind: "none",
      headersJson: "{}",
      enabled: true,
      policy: "ask",
      protocolVersion: "2026-07-28",
      createdAt: now(),
      lastStatus: "connected",
      lastError: null,
      lastCheckedAt: now(),
      serverInfoJson: JSON.stringify({ name: "mock-modern", version: "1.0.0" }),
      capabilitiesJson: JSON.stringify({ tools: {} }),
      toolsJson: JSON.stringify([TOOL]),
      toolsCachedAt: now(),
      oauthIssuer: null,
      oauthClientId: null,
      negotiatedAt: now(),
      hasBearer: false,
      hasApiKey: false,
      toolCount: 1,
    },
  ];

  function emit(event: CoreEvent) {
    for (const subscriber of subscribers) subscriber(event);
  }

  function addMessage(conversationId: string, message: MessageWithParts) {
    const list = messages.get(conversationId) ?? [];
    list.push(message);
    messages.set(conversationId, list);
  }

  function textPart(messageId: string, text: string): Part {
    return {
      id: uid(),
      messageId,
      idx: 0,
      kind: "text",
      payloadJson: JSON.stringify({ text }),
    };
  }

  function plainText(parts: Part[]): string {
    return parts
      .filter((part) => part.kind === "text")
      .map((part) => {
        try {
          return String((JSON.parse(part.payloadJson) as { text?: unknown }).text ?? "");
        } catch {
          return "";
        }
      })
      .join("");
  }

  if (!empty) {
    const seeded: Conversation = {
      id: "demo",
      title: "Welcome to Jarvis",
      createdAt: now() - 1000 * 60 * 8,
      updatedAt: now() - 1000 * 60 * 8,
      providerId: "provider-mock",
      model: "mock-text",
      workspaceRoot: "/home/you/project",
      systemPrompt: null,
      archived: false,
      pinned: false,
      maxSteps: 25,
      toolMode: "lazy",
      serversConfigured: false,
    };
    conversations.push(seeded);
    const assistantId = "demo-assistant";
    messages.set("demo", [
      {
        id: "demo-user",
        conversationId: "demo",
        role: "user",
        createdAt: now() - 1000 * 60 * 7,
        status: "completed",
        error: null,
        parts: [textPart("demo-user", "What can you do with MCP tools?")],
      },
      {
        id: assistantId,
        conversationId: "demo",
        role: "assistant",
        createdAt: now() - 1000 * 60 * 6,
        status: "completed",
        error: null,
        parts: [
          {
            id: "demo-thought",
            messageId: assistantId,
            idx: 0,
            kind: "reasoning",
            payloadJson: JSON.stringify({
              text: "The user wants to know what MCP tools can do. I should demonstrate one read-only call, then show how a file change is summarized.",
            }),
          },
          textPart(
            assistantId,
            "I can call tools exposed by remote MCP servers. Let me show you with the echo tool.",
          ),
          {
            id: "demo-tool",
            messageId: assistantId,
            idx: 1,
            kind: "tool_call",
            payloadJson: JSON.stringify({
              callId: "call_demo",
              name: "echo",
              server: "Mock MCP",
              args: { text: "hello from MCP" },
              state: "completed",
              startedAt: now() - 1000 * 60 * 6,
              endedAt: now() - 1000 * 60 * 6 + 340,
              executed: true,
              approval: "allowed",
              resultPreview: "echo: hello from MCP",
              truncated: false,
            }),
          },
          {
            id: "demo-edit",
            messageId: assistantId,
            idx: 2,
            kind: "tool_call",
            payloadJson: JSON.stringify({
              callId: "call_demo_edit",
              name: "edit",
              server: "mcp-chat",
              args: {
                filePath: "src/app.ts",
                oldString: "const greeting = \"hello\";",
                newString: "const greeting = \"hello, MCP\";\nconst tools = 1;",
              },
              state: "completed",
              startedAt: now() - 1000 * 60 * 6 + 400,
              endedAt: now() - 1000 * 60 * 6 + 520,
              executed: true,
              approval: "allowed",
              resultPreview: "Edited src/app.ts: replaced 1 occurrence(s).",
              truncated: false,
              diff: [
                "--- a/src/app.ts",
                "+++ b/src/app.ts",
                "@@ -1,4 +1,5 @@",
                " import { run } from \"./run\";",
                "-const greeting = \"hello\";",
                "+const greeting = \"hello, MCP\";",
                "+const tools = 1;",
                " run(greeting);",
                "",
              ].join("\n"),
            }),
          },
          {
            id: "demo-write",
            messageId: assistantId,
            idx: 3,
            kind: "tool_call",
            payloadJson: JSON.stringify({
              callId: "call_demo_write",
              name: "write",
              server: "mcp-chat",
              args: { filePath: "docs/notes.md", content: "# Notes\n\nWritten by the write tool.\n" },
              state: "completed",
              startedAt: now() - 1000 * 60 * 6 + 540,
              endedAt: now() - 1000 * 60 * 6 + 600,
              executed: true,
              approval: "allowed",
              resultPreview: "Wrote docs/notes.md (36 bytes).",
              truncated: false,
              diff: [
                "--- a/docs/notes.md",
                "+++ b/docs/notes.md",
                "@@ -0,0 +1,3 @@",
                "+# Notes",
                "+",
                "+Written by the write tool.",
                "",
              ].join("\n"),
            }),
          },
          {
            id: "demo-patch",
            messageId: assistantId,
            idx: 4,
            kind: "tool_call",
            payloadJson: JSON.stringify({
              callId: "call_demo_patch",
              name: "apply_patch",
              server: "mcp-chat",
              args: {
                patchText: [
                  "*** Begin Patch",
                  "*** Update File: src/util.ts",
                  "@@",
                  " export function add(a: number, b: number) {",
                  "-  return a + b;",
                  "+  return a + b + 0;",
                  " }",
                  "*** Add File: src/index.ts",
                  "+export * from \"./util\";",
                  "*** End Patch",
                ].join("\n"),
              },
              state: "completed",
              startedAt: now() - 1000 * 60 * 6 + 620,
              endedAt: now() - 1000 * 60 * 6 + 780,
              executed: true,
              approval: "allowed",
              resultPreview: "Applied patch (2 file(s)).\nM src/util.ts (+1/-1)\nA src/index.ts (+1)",
              truncated: false,
              diff: [
                "--- a/src/util.ts",
                "+++ b/src/util.ts",
                "@@ -1,3 +1,3 @@",
                " export function add(a: number, b: number) {",
                "-  return a + b;",
                "+  return a + b + 0;",
                " }",
                "--- a/src/index.ts",
                "+++ b/src/index.ts",
                "@@ -0,0 +1 @@",
                "+export * from \"./util\";",
                "",
              ].join("\n"),
            }),
          },
        ],
      },
      {
        id: "demo-final",
        conversationId: "demo",
        role: "assistant",
        createdAt: now() - 1000 * 60 * 5,
        status: "completed",
        error: null,
        parts: [
          textPart(
            "demo-final",
            "The tool returned `echo: hello from MCP`. Thoughts and tool calls appear above as compact rows — click a row to inspect its arguments, result and file changes.",
          ),
        ],
      },
    ]);
  }

  const bridge: MockBridge = {
    async invoke<T>(command: string, args: Record<string, unknown> = {}): Promise<T> {
      const value = await handle(command, args);
      return value as T;
    },
    subscribe(handler) {
      subscribers.add(handler);
      return () => subscribers.delete(handler);
    },
  };

  async function handle(command: string, args: Record<string, unknown>): Promise<unknown> {
    switch (command) {
      case "app_info":
        return {
          name: "Jarvis",
          version: "0.1.0-mock",
          dataDir: "/home/you/.local/share/com.mcpchat.app",
          workspaceDir: "/home/you/project",
          platform: "linux",
        };
      case "list_settings":
        return { ...settingsStore };
      case "set_setting":
        settingsStore[String(args.key)] = String(args.value);
        if (args.key === "theme") localStorage.setItem("theme", String(args.value));
        return null;
      case "list_conversations": {
        const offset = Number(args.offset ?? 0);
        const limit = Number(args.limit ?? 200);
        return [...conversations]
          .sort((a, b) => Number(b.pinned) - Number(a.pinned) || b.updatedAt - a.updatedAt)
          .slice(offset, offset + limit);
      }
      case "search_conversations": {
        const query = String(args.query ?? "").toLowerCase();
        return conversations
          .filter((conversation) => conversation.title.toLowerCase().includes(query))
          .map((conversation) => ({
            conversationId: conversation.id,
            title: conversation.title,
            snippet: "",
            updatedAt: conversation.updatedAt,
          }));
      }
      case "get_conversation":
        return conversations.find((conversation) => conversation.id === args.conversationId) ?? null;
      case "create_conversation": {
        const conversation: Conversation = {
          id: uid(),
          title: "New chat",
          createdAt: now(),
          updatedAt: now(),
          providerId: providers[0]?.id ?? null,
          model: providers[0]?.model ?? null,
          workspaceRoot:
            (args.input as { workspaceRoot?: string | null } | undefined)?.workspaceRoot ??
            "/home/you/project",
          systemPrompt: null,
          archived: false,
          pinned: false,
          maxSteps: 25,
          toolMode: "lazy",
          serversConfigured: false,
        };
        conversations.unshift(conversation);
        messages.set(conversation.id, []);
        return conversation;
      }
      case "rename_conversation": {
        const conversation = conversations.find((item) => item.id === args.conversationId);
        if (conversation) {
          conversation.title = String(args.title ?? conversation.title);
          conversation.updatedAt = now();
        }
        return null;
      }
      case "set_conversation_pinned": {
        const conversation = conversations.find((item) => item.id === args.conversationId);
        if (conversation) conversation.pinned = Boolean(args.pinned);
        return null;
      }
      case "delete_conversation": {
        const index = conversations.findIndex((item) => item.id === args.conversationId);
        if (index >= 0) conversations.splice(index, 1);
        messages.delete(String(args.conversationId));
        autoApprove.delete(String(args.conversationId));
        return null;
      }
      case "get_messages_page": {
        const all = messages.get(String(args.conversationId)) ?? [];
        const before = typeof args.before === "string" ? args.before : null;
        let end = before ? all.findIndex((message) => message.id === before) : all.length;
        if (end < 0) end = all.length;
        const limit = Math.min(500, Math.max(1, Number(args.limit ?? 80)));
        const start = Math.max(0, end - limit);
        return { messages: all.slice(start, end), hasMore: start > 0 };
      }
      case "get_messages":
        return messages.get(String(args.conversationId)) ?? [];
      case "get_conversation_tools": {
        const conversation = conversations.find(
          (item) => item.id === args.conversationId,
        );
        return {
          serverIds: servers.filter((server) => server.enabled).map((server) => server.id),
          allServers: !(conversation?.serversConfigured ?? false),
          toolMode: conversation?.toolMode ?? "direct",
        };
      }
      case "set_conversation_workspace": {
        const conversation = conversations.find(
          (item) => item.id === args.conversationId,
        );
        if (conversation) conversation.workspaceRoot = String(args.workspaceRoot);
        return String(args.workspaceRoot);
      }
      case "set_conversation_tools": {
        const conversation = conversations.find(
          (item) => item.id === args.conversationId,
        );
        if (conversation) {
          conversation.toolMode = String(args.toolMode ?? "direct") as
            | "direct"
            | "lazy";
          conversation.serversConfigured = args.serverIds != null;
        }
        return null;
      }
      case "context_status":
        return {
          tokens: 12450,
          window: 128000,
          threshold: 108800,
          keepTokens: 15000,
          toolCount: 17,
          toolMode: "lazy",
        };
      case "compact_conversation":
        return null;
      case "is_generating":
        return generating.has(String(args.conversationId));
      case "get_auto_approve":
        return autoApprove.has(String(args.conversationId));
      case "set_auto_approve": {
        const conversationId = String(args.conversationId);
        if (args.enabled) autoApprove.add(conversationId);
        else autoApprove.delete(conversationId);
        return autoApprove.has(conversationId);
      }
      case "stop_generation": {
        generating.delete(String(args.conversationId));
        emit({
          type: "status",
          conversationId: String(args.conversationId),
          status: "idle",
          message: "Stopped.",
        });
        return true;
      }
      case "steer_message":
      case "send_message": {
        const conversationId = String(args.conversationId);
        const text = String(args.text ?? "").trim();
        const user: MessageWithParts = {
          id: uid(),
          conversationId,
          role: "user",
          createdAt: now(),
          status: "completed",
          error: null,
          parts: [textPart("user", text)],
        };
        addMessage(conversationId, user);
        const conversation = conversations.find((item) => item.id === conversationId);
        if (conversation && conversation.title === "New chat") {
          conversation.title = text.slice(0, 60) || "New chat";
        }
        if (conversation) conversation.updatedAt = now();
        emit({ type: "conversation_updated", conversationId });
        generating.add(conversationId);
        void simulateTurn(conversationId, text);
        return user;
      }
      case "regenerate":
      case "continue_turn":
        return null;
      case "revert_message": {
        const conversationId = String(args.conversationId);
        if (generating.has(conversationId)) {
          throw new Error("Stop the current turn before reverting a message.");
        }
        const list = messages.get(conversationId) ?? [];
        const index = list.findIndex(
          (message) =>
            message.id === String(args.messageId) && message.role === "user",
        );
        if (index < 0) throw new Error("Message to revert was not found.");
        const target = list[index];
        if (!target) throw new Error("Message to revert was not found.");
        const text = plainText(target.parts).trim();
        messages.set(conversationId, list.slice(0, index));
        const conversation = conversations.find((item) => item.id === conversationId);
        if (conversation) {
          conversation.updatedAt = now();
          if ((messages.get(conversationId) ?? []).length === 0) {
            conversation.title = "New chat";
          }
        }
        emit({ type: "conversation_updated", conversationId });
        return { text, attachments: [] };
      }
      case "respond_approval": {
        emit({
          type: "tool",
          conversationId: String(args.conversationId ?? ""),
          callId: String(args.callId),
          state: args.decision === "reject" ? "error" : "running",
          message: null,
        });
        return true;
      }
      case "respond_input":
        return true;
      case "read_tool_output":
        return JSON.stringify({
          content: "Full mock tool output. ".repeat(80),
          offset: Number(args.offset ?? 0),
          next_offset: 1600,
          total: 1600,
        });
      case "export_conversation": {
        const conversation = conversations.find((item) => item.id === args.conversationId);
        const content =
          String(args.format) === "json"
            ? JSON.stringify(
                { conversation, messages: messages.get(String(args.conversationId)) ?? [] },
                null,
                2,
              )
            : `# ${conversation?.title ?? "Conversation"}\n\nExported from Jarvis.`;
        return {
          filename: `${conversation?.title ?? "conversation"}.${String(args.format) === "json" ? "json" : "md"}`,
          content,
          mime: String(args.format) === "json" ? "application/json" : "text/markdown",
        } satisfies ExportResult;
      }
      case "list_providers":
        return providers;
      case "create_provider": {
        const input = (args.input ?? {}) as Record<string, unknown>;
        const provider: ProviderView = {
          id: uid(),
          name: String(input.name),
          kind: String(input.kind),
          baseUrl: String(input.baseUrl),
          model: String(input.model),
          apiKeyRef: null,
          contextWindow: Number(input.contextWindow ?? 128000),
          maxOutputTokens: Number(input.maxOutputTokens ?? 8192),
          createdAt: now(),
          updatedAt: now(),
          hasKey: Boolean(input.apiKey),
          keyHint: input.apiKey ? "••••" : null,
          models: (input.models as string[] | undefined) ?? [],
        };
        providers = [...providers, provider];
        return provider;
      }
      case "update_provider": {
        const input = (args.input ?? {}) as Record<string, unknown>;
        providers = providers.map((provider) =>
          provider.id === args.providerId
            ? { ...provider, ...input, apiKey: undefined, hasKey: provider.hasKey } as ProviderView
            : provider,
        );
        return providers.find((provider) => provider.id === args.providerId);
      }
      case "set_provider_key": {
        providers = providers.map((provider) =>
          provider.id === args.providerId
            ? { ...provider, hasKey: true, keyHint: "••••" }
            : provider,
        );
        return providers.find((provider) => provider.id === args.providerId);
      }
      case "clear_provider_key":
      case "delete_provider":
      case "test_provider":
        return {
          ok: true,
          models: ["mock-text", "mock-tool", "mock-tool"],
          message: "Connected. 3 model(s) available.",
        };
      case "fetch_models":
      case "fetch_models_for_config":
        return ["mock-text", "mock-tool", "mock-parallel"];
      case "list_servers":
        return servers;
      case "create_server":
      case "update_server": {
        const input = (args.input ?? {}) as Record<string, unknown>;
        const server: ServerView = {
          ...(servers[0] as ServerView),
          id: uid(),
          name: String(input.name ?? "New MCP server"),
          slug: String(input.name ?? "server").toLowerCase().replace(/[^a-z0-9]+/g, "_"),
          url: String(input.url ?? ""),
          policy: String(input.policy ?? "ask"),
          authKind: String(input.authKind ?? "none"),
          headersJson: String(input.headersJson ?? "{}"),
          lastStatus: "connected",
          toolsJson: JSON.stringify([TOOL]),
          toolCount: 1,
          hasBearer: Boolean(input.bearer),
          hasApiKey: Boolean(input.apiKey),
        };
        servers.push(server);
        return server;
      }
      case "set_server_enabled":
        return null;
      case "set_server_bearer":
        return null;
      case "delete_server":
        return null;
      case "test_server":
      case "refresh_server":
        return {
          protocolVersion: "2026-07-28",
          transport: "streamable_http",
          serverInfo: { name: "mock-modern", version: "1.0.0" },
          capabilities: { tools: {} },
          instructions: "Mock MCP server for mcp-chat tests",
          tools: [TOOL],
        } satisfies ConnectionReport;
      case "refresh_all_servers":
        return null;
      case "list_server_tools":
        return [TOOL];
      case "run_server_tool":
        return {
          content: [{ type: "text", text: `echo: ${JSON.stringify(args.arguments)}` }],
          isError: false,
        };
      case "list_permission_rules":
        return [];
      case "delete_permission_rule":
        return null;
      case "delete_all_data":
        conversations.splice(0);
        messages.clear();
        return null;
      case "open_external":
      case "open_data_dir":
        return null;
      default:
        throw new Error(`Mock bridge does not implement ${command}`);
    }
  }

  async function simulateTurn(conversationId: string, text: string) {
    await delay(160);
    const assistantId = uid();
    const assistant: MessageWithParts = {
      id: assistantId,
      conversationId,
      role: "assistant",
      createdAt: now(),
      status: "streaming",
      error: null,
      parts: [],
    };
    addMessage(conversationId, assistant);
    emit({ type: "message_start", conversationId, messageId: assistantId });
    emit({ type: "status", conversationId, status: "streaming", message: null });
    emit({
      type: "usage",
      conversationId,
      promptTokens: 1840,
      completionTokens: 0,
      contextWindow: 128000,
    });
    // "long thought" streams a tall reasoning part so scrolling can be exercised.
    let thoughtText = "";
    const thoughtId = uid();
    if (/long thought/i.test(text)) {
      for (let step = 1; step <= 200; step += 1) {
        if (!generating.has(conversationId)) return;
        const line = `Step ${step}: weighing the next option against the constraints.\n`;
        thoughtText += line;
        emit({
          type: "delta",
          conversationId,
          messageId: assistantId,
          partId: thoughtId,
          deltaType: "reasoning",
          text: line,
        });
        await delay(35);
      }
      emit({
        type: "part",
        conversationId,
        messageId: assistantId,
        partId: thoughtId,
        kind: "reasoning",
        payload: { text: thoughtText },
      });
    }
    const partId = uid();
    const interim = "Let me check that with the echo tool.";
    let buffer = "";
    for (const word of interim.split(" ")) {
      if (!generating.has(conversationId)) return;
      buffer += (buffer ? " " : "") + word;
      emit({
        type: "delta",
        conversationId,
        messageId: assistantId,
        partId,
        deltaType: "text",
        text: word + " ",
      });
      await delay(28);
    }
    emit({
      type: "part",
      conversationId,
      messageId: assistantId,
      partId,
      kind: "text",
      payload: { text: buffer },
    });

    const callId = uid();
    const toolPartId = uid();
    const startedAt = now();
    emit({
      type: "part",
      conversationId,
      messageId: assistantId,
      partId: toolPartId,
      kind: "tool_call",
      payload: {
        callId,
        name: "echo",
        server: "Mock MCP",
        args: { text: text.slice(0, 60) },
        state: "running",
        startedAt,
        executed: false,
      },
    });
    emit({
      type: "tool",
      conversationId,
      callId,
      state: "running",
      progress: 0.4,
      message: "Calling tool",
    });
    await delay(520);
    emit({
      type: "tool",
      conversationId,
      callId,
      state: "completed",
      progress: null,
      message: `echo: ${text}`,
    });
    const list = messages.get(conversationId) ?? [];
    const target = list.find((message) => message.id === assistantId);
    if (thoughtText) {
      target?.parts.push({
        id: thoughtId,
        messageId: assistantId,
        idx: 0,
        kind: "reasoning",
        payloadJson: JSON.stringify({ text: thoughtText }),
      });
    }
    target?.parts.push({
      id: partId,
      messageId: assistantId,
      idx: 0,
      kind: "text",
      payloadJson: JSON.stringify({ text: buffer }),
    });
    target?.parts.push({
      id: toolPartId,
      messageId: assistantId,
      idx: 1,
      kind: "tool_call",
      payloadJson: JSON.stringify({
        callId,
        name: "echo",
        server: "Mock MCP",
        args: { text: text.slice(0, 60) },
        state: "completed",
        startedAt,
        endedAt: now(),
        executed: true,
        approval: "allowed",
        resultPreview: `echo: ${text}`,
        truncated: false,
      }),
    });
    if (target) target.status = "completed";
    emit({
      type: "message_end",
      conversationId,
      messageId: assistantId,
      status: "completed",
      error: null,
    });

    const finalId = uid();
    const finalPartId = uid();
    const finalMessage: MessageWithParts = {
      id: finalId,
      conversationId,
      role: "assistant",
      createdAt: now(),
      status: "streaming",
      error: null,
      parts: [],
    };
    addMessage(conversationId, finalMessage);
    emit({ type: "message_start", conversationId, messageId: finalId });
    emit({ type: "status", conversationId, status: "streaming", message: null });
    const finalText = `I received **${text}**. This is the mock IPC bridge, so the response is scripted. Tool cards, approvals and errors are rendered by the same UI that runs in Tauri.`;
    let finalBuffer = "";
    for (const word of finalText.split(" ")) {
      if (!generating.has(conversationId)) return;
      finalBuffer += (finalBuffer ? " " : "") + word;
      emit({
        type: "delta",
        conversationId,
        messageId: finalId,
        partId: finalPartId,
        deltaType: "text",
        text: word + " ",
      });
      await delay(24);
    }
    finalMessage.parts.push({
      id: finalPartId,
      messageId: finalId,
      idx: 0,
      kind: "text",
      payloadJson: JSON.stringify({ text: finalBuffer }),
    });
    finalMessage.status = "completed";
    generating.delete(conversationId);
    emit({
      type: "message_end",
      conversationId,
      messageId: finalId,
      status: "completed",
      error: null,
    });
    emit({
      type: "usage",
      conversationId,
      promptTokens: 1840,
      completionTokens: 96,
      contextWindow: 128000,
    });
    emit({ type: "status", conversationId, status: "idle", message: null });
    const conversation = conversations.find((item) => item.id === conversationId);
    if (conversation) conversation.updatedAt = now();
    emit({ type: "conversation_updated", conversationId });
  }

  window.__MCP_CHAT_MOCK__ = bridge;
}

function delay(ms: number): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, ms));
}
