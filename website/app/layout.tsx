import type { Metadata } from "next";
import { Geist, Geist_Mono } from "next/font/google";
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
  metadataBase: new URL("https://open-agent-view.github.io"),
  title: "Open Agent View — One control surface for local coding agents",
  description:
    "Discover, follow, and safely control local coding-agent sessions from one terminal.",
  alternates: { canonical: "/" },
  icons: { icon: "/favicon.svg" },
  openGraph: {
    type: "website",
    url: "/",
    title: "Open Agent View",
    description: "One control surface for all your local coding agents.",
    images: [{ url: "/og.png", width: 1200, height: 630, alt: "Open Agent View" }],
  },
  twitter: {
    card: "summary_large_image",
    title: "Open Agent View",
    description: "One control surface for all your local coding agents.",
    images: ["/og.png"],
  },
};

export default function RootLayout({
  children,
}: Readonly<{
  children: React.ReactNode;
}>) {
  return (
    <html lang="en" suppressHydrationWarning>
      <head>
        {/* The player is a vendored static runtime asset, not a framework stylesheet. */}
        {/* eslint-disable-next-line @next/next/no-css-tags */}
        <link rel="stylesheet" href="/asciinema-player.css" />
      </head>
      <body
        className={`${geistSans.variable} ${geistMono.variable} antialiased`}
      >
        {children}
        <script src="/asciinema-player.min.js" defer />
        <script src="/site.js" defer />
      </body>
    </html>
  );
}
