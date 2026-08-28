import type { Metadata } from "next";
import { Geist, Geist_Mono } from "next/font/google";
import "./globals.css";
import harnessCatalog from "./harnesses.json";

const codingHarnessCount = harnessCatalog.codingHarnesses.length;
const description = `Monitor ${codingHarnessCount} coding harnesses plus Terminal, see what needs input, and jump into each native CLI from one live dashboard.`;

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
  title: "Open Agent View: One dashboard for every coding agent",
  description,
  alternates: { canonical: "/" },
  icons: { icon: "/favicon.svg" },
  openGraph: {
    type: "website",
    url: "/",
    title: "Open Agent View: One dashboard for every coding agent",
    description,
    images: [{
      url: "/og.png",
      width: 1200,
      height: 630,
      alt: `Open Agent View dashboard for ${codingHarnessCount} coding harnesses plus Terminal`,
    }],
  },
  twitter: {
    card: "summary_large_image",
    title: "Open Agent View: One dashboard for every coding agent",
    description,
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
