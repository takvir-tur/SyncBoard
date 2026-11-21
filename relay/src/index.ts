import { DurableObject } from "cloudflare:workers";

export interface Env {
  SYNC_ROOM: DurableObjectNamespace<SyncRoom>;
}

const PROTOCOL_VERSION = 1;
const HEADER_LEN = 26;
const NONCE_LEN = 12;
const AEAD_TAG_LEN = 16;
const MIN_FRAME_LEN = HEADER_LEN + NONCE_LEN + AEAD_TAG_LEN;
const MAX_FRAME_LEN = 9 * 1024 * 1024;

const TAG_EPHEMERAL_TEXT = 1;
const TAG_EPHEMERAL_IMAGE = 2;
const TAG_PIN_UPSERT = 3;
const TAG_PIN_DELETE = 4;

const MAX_PINS_PER_ROOM = 25;

function bytesToHex(bytes: Uint8Array): string {
  let out = "";
  for (let i = 0; i < bytes.length; i++) {
    out += bytes[i].toString(16).padStart(2, "0");
  }
  return out;
}

function readUint64BE(view: DataView, offset: number): number {
  return Number(view.getBigUint64(offset, false));
}

export class SyncRoom extends DurableObject<Env> {
  constructor(ctx: DurableObjectState, env: Env) {
    super(ctx, env);

    // Handle keepalive pings without waking a hibernated Durable Object.
    this.ctx.setWebSocketAutoResponse(new WebSocketRequestResponsePair("ping", "pong"));

    this.ctx.storage.sql.exec(`
      CREATE TABLE IF NOT EXISTS pins (
        pin_id TEXT PRIMARY KEY,
        updated_at INTEGER NOT NULL,
        frame BLOB NOT NULL
      )
    `);
  }

  async fetch(_request: Request): Promise<Response> {
    const pair = new WebSocketPair();
    const client = pair[0];
    const server = pair[1];

    this.ctx.acceptWebSocket(server);

    // Replay encrypted pin blobs to newly connected / reconnecting peers.
    const rows = this.ctx.storage.sql
      .exec<{ frame: ArrayBuffer }>(
        "SELECT frame FROM pins ORDER BY updated_at ASC LIMIT ?",
        MAX_PINS_PER_ROOM
      )
      .toArray();

    for (const row of rows) {
      try {
        server.send(row.frame);
      } catch {
        break;
      }
    }

    return new Response(null, { status: 101, webSocket: client });
  }

  async webSocketMessage(ws: WebSocket, message: string | ArrayBuffer): Promise<void> {
    if (typeof message === "string") {
      return;
    }

    if (message.byteLength < MIN_FRAME_LEN || message.byteLength > MAX_FRAME_LEN) {
      return;
    }

    const bytes = new Uint8Array(message);
    const version = bytes[0];
    const tag = bytes[1];
    if (
      version !== PROTOCOL_VERSION ||
      (tag !== TAG_EPHEMERAL_TEXT &&
        tag !== TAG_EPHEMERAL_IMAGE &&
        tag !== TAG_PIN_UPSERT &&
        tag !== TAG_PIN_DELETE)
    ) {
      return;
    }

    const pinId = bytesToHex(bytes.subarray(2, 18));
    const updatedAt = readUint64BE(new DataView(message), 18);

    if (tag === TAG_PIN_UPSERT) {
      this.ctx.storage.sql.exec(
        "INSERT OR REPLACE INTO pins (pin_id, updated_at, frame) VALUES (?, ?, ?)",
        pinId,
        updatedAt,
        bytes
      );
      this.ctx.storage.sql.exec(
        `DELETE FROM pins
         WHERE pin_id NOT IN (
           SELECT pin_id FROM pins ORDER BY updated_at DESC, pin_id DESC LIMIT ?
         )`,
        MAX_PINS_PER_ROOM
      );
    } else if (tag === TAG_PIN_DELETE) {
      this.ctx.storage.sql.exec("DELETE FROM pins WHERE pin_id = ?", pinId);
    }

    for (const peer of this.ctx.getWebSockets()) {
      if (peer === ws) {
        continue;
      }
      try {
        peer.send(message);
      } catch {
        try {
          peer.close(1011, "peer send error");
        } catch {
          // Ignore already-closed sockets.
        }
      }
    }
  }

  async webSocketClose(
    ws: WebSocket,
    code: number,
    _reason: string,
    _wasClean: boolean
  ): Promise<void> {
    try {
      ws.close(code, "closed");
    } catch {
      // Ignore already-closed sockets.
    }
  }

  async webSocketError(ws: WebSocket, _error: unknown): Promise<void> {
    try {
      ws.close(1011, "websocket error");
    } catch {
      // Ignore already-closed sockets.
    }
  }
}

export default {
  async fetch(request: Request, env: Env): Promise<Response> {
    const url = new URL(request.url);

    if (request.method === "GET" && url.pathname === "/health") {
      return Response.json({ ok: true });
    }

    const match = /^\/room\/([0-9a-f]{64})$/i.exec(url.pathname);
    if (!match) {
      return new Response("Not Found", { status: 404 });
    }

    if (request.headers.get("Upgrade")?.toLowerCase() !== "websocket") {
      return new Response("Expected WebSocket Upgrade", { status: 426 });
    }

    const roomId = match[1].toLowerCase();
    const stub = env.SYNC_ROOM.get(env.SYNC_ROOM.idFromName(roomId));
    return stub.fetch(request);
  },
};
