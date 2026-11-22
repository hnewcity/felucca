// Keep one real public connection open while exercising console mutations.
import { connect } from "node:net";
import { writeFile } from "node:fs/promises";
import { resolve } from "node:path";
const port = Number(process.argv[2]);
const output = resolve(process.argv[3]);
if (!Number.isInteger(port) || port < 1 || port > 65535) throw new Error("A port is required");
let stopping = false, replies = 0, buffer = "";
const socket = connect({ host: "127.0.0.1", port });
const request = () => socket.write("GET / HTTP/1.1\r\nHost: localhost\r\nConnection: keep-alive\r\n\r\n");
socket.on("connect", request);
socket.on("data", (bytes) => {
  buffer += bytes.toString();
  if (buffer.includes("felucca-guided-flow\n") && buffer.endsWith("0\r\n\r\n")) {
    replies++;
    buffer = "";
    if (replies === 1) console.log("One connection is open and carrying real HTTP responses.");
    if (!stopping) setTimeout(request, 500).unref();
  }
});
socket.on("error", () => {});
socket.on("close", async () => {
  const passed = stopping && replies > 2;
  await writeFile(output, JSON.stringify({ status: passed ? "passed" : "failed", replies, connections: 1, reconnected: false }, null, 2));
  process.exitCode = passed ? 0 : 1;
});
for (const signal of ["SIGINT", "SIGTERM"]) process.once(signal, () => {
  stopping = true;
  socket.end();
});
