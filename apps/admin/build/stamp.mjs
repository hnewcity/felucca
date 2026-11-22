import { createHash } from "node:crypto";
import { readdir, readFile, writeFile } from "node:fs/promises";

async function files(directory) {
  const entries = await readdir(directory, { withFileTypes: true });
  return (
    await Promise.all(
      entries.map((entry) => {
        const path = `${directory}/${entry.name}`;
        return entry.isDirectory() ? files(path) : path;
      }),
    )
  ).flat();
}
const inputs = [
  "package.json",
  "package-lock.json",
  "index.html",
  "vite.config.js",
  ...(await files("src")),
  ...(await files("public")),
  ...(await files("build")),
].sort();
const hash = createHash("sha256");
for (const path of inputs)
  hash
    .update(path)
    .update("\0")
    .update(await readFile(path))
    .update("\0");
await writeFile("dist/.source-hash", `${hash.digest("hex")}\n`);
