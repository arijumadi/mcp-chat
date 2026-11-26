// Minimal stdio ACP v1 mock agent for the Phase 0 spike.
// It speaks newline-delimited JSON-RPC 2.0 and keeps one session alive across
// two prompts. No external dependencies.

import readline from "node:readline";

const sessionId = "phase0-session";
let turn = 0;

const rl = readline.createInterface({
  input: process.stdin,
  crlfDelay: Infinity,
});

function send(message) {
  process.stdout.write(`${JSON.stringify(message)}\n`);
}

function textFromPrompt(params) {
  const prompt = params?.prompt;
  if (!Array.isArray(prompt)) return "";
  return prompt
    .map((block) => (block?.type === "text" ? String(block.text ?? "") : ""))
    .join("");
}

function update(text) {
  send({
    jsonrpc: "2.0",
    method: "session/update",
    params: {
      sessionId,
      update: {
        sessionUpdate: "agent_message_chunk",
        content: { type: "text", text },
      },
    },
  });
}

rl.on("line", (line) => {
  if (!line.trim()) return;

  let message;
  try {
    message = JSON.parse(line);
  } catch {
    return;
  }

  if (message.method === "initialize") {
    send({
      jsonrpc: "2.0",
      id: message.id,
      result: {
        protocolVersion: 1,
        agentCapabilities: { loadSession: false },
        authMethods: [],
        agentInfo: { name: "mock-acp-phase0", version: "0.0.0" },
      },
    });
    return;
  }

  if (message.method === "session/new") {
    const cwd = message.params?.cwd ?? "";
    send({
      jsonrpc: "2.0",
      id: message.id,
      result: { sessionId, _meta: { cwd } },
    });
    return;
  }

  if (message.method === "session/prompt") {
    turn += 1;
    const prompt = textFromPrompt(message.params);
    update(`turn ${turn} received ${prompt || "<empty>"}`);
    send({
      jsonrpc: "2.0",
      id: message.id,
      result: { stopReason: "end_turn" },
    });
    return;
  }

  if (message.method === "session/close") {
    send({ jsonrpc: "2.0", id: message.id, result: {} });
    setTimeout(() => process.exit(0), 25);
    return;
  }

  if (message.method === "session/cancel") {
    return;
  }

  if (message.id !== undefined) {
    send({
      jsonrpc: "2.0",
      id: message.id,
      error: { code: -32601, message: `method not found: ${message.method}` },
    });
  }
});

rl.on("close", () => process.exit(0));
