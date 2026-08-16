import net from "node:net";

const port = Number(process.argv[2] || 1420);
const host = "127.0.0.1";

const server = net.createServer();
server.once("error", (err) => {
  const code = err && typeof err === "object" && "code" in err ? err.code : err;
  console.error(
    `Port ${port} is already in use (${code}). Stop the other process, then retry.\n` +
      `Tauri loads this port from src-tauri/tauri.conf.json — Next.js must not hop to another port.`,
  );
  process.exit(1);
});
server.listen(port, host, () => {
  server.close(() => process.exit(0));
});
