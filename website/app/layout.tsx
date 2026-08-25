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
    <html lang="en">
      <body
        className={`${geistSans.variable} ${geistMono.variable} antialiased`}
      >
        {children}
        <script src="/site.js" defer />
      </body>
    </html>
  );
}
