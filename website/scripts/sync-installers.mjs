import { chmod, copyFile } from "node:fs/promises";

const website = new URL("../", import.meta.url);
const repository = new URL("../../", import.meta.url);

await Promise.all(
  ["install.sh", "install.ps1"].map(async (name) => {
    const destination = new URL(`public/${name}`, website);
    await copyFile(new URL(name, repository), destination);
    await chmod(destination, 0o644);
  }),
);
