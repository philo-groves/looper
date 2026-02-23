import type { Metadata } from "next";
import { Geist, Geist_Mono } from "next/font/google";

import { WorkspaceShell } from "@/components/window/WorkspaceShell";

import "./globals.css";

const geistSans = Geist({
  variable: "--font-geist-sans",
  subsets: ["latin"],
});

const geistMono = Geist_Mono({
  variable: "--font-geist-mono",
  subsets: ["latin"],
});

export const metadata: Metadata = {
  title: "Looper Dashboard",
  description: "Interactive dashboard for Looper agent runtime",
};

export default function RootLayout({
  children,
}: Readonly<{
  children: React.ReactNode;
}>) {
  return (
    <html lang="en" suppressHydrationWarning>
      <body
        className={`${geistSans.variable} ${geistMono.variable} bg-white text-black antialiased dark:bg-black dark:text-white`}
      >
        <WorkspaceShell>{children}</WorkspaceShell>
      </body>
    </html>
  );
}
