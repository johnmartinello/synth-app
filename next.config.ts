const isProd = process.env.NODE_ENV === "production";
const internalHost = process.env.TAURI_DEV_HOST || "127.0.0.1";
const devPort = process.env.PORT || "1420";

const nextConfig = {
  // Static export for Tauri (no Node SSR in the desktop app).
  output: "export" as const,
  images: {
    unoptimized: true,
  },
  // Keep this on the same host/port as tauri.conf.json `devUrl`.
  // Port 3000 is often taken by other Next apps; Tauri will still load it.
  allowedDevOrigins: ["127.0.0.1", "localhost"],
  assetPrefix: isProd ? undefined : `http://${internalHost}:${devPort}`,
};

export default nextConfig;
