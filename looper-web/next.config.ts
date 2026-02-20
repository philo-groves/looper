import type { NextConfig } from "next";

const nextConfig: NextConfig = {
  async rewrites() {
    const agentBaseUrl =
      process.env.LOOPER_AGENT_URL ?? "http://127.0.0.1:10001";

    return [
      {
        source: "/api/agent/:path*",
        destination: `${agentBaseUrl}/api/:path*`,
      },
    ];
  },
};

export default nextConfig;
