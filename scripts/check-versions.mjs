import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const read = (name) => fs.readFileSync(path.join(root, name), "utf8");

const packageVersion = JSON.parse(read("package.json")).version;
const tauriVersion = JSON.parse(read("src-tauri/tauri.conf.json")).version;
const cargo = read("Cargo.toml");
const workspace = cargo.split(/^\[workspace\.package\]\s*$/m)[1]?.split(/^\[/m)[0] ?? "";
const cargoVersion = workspace.match(/^version\s*=\s*"([^"]+)"/m)?.[1];

const versions = {
  "package.json": packageVersion,
  "Cargo.toml [workspace.package]": cargoVersion,
  "src-tauri/tauri.conf.json": tauriVersion,
};

const mismatched = Object.entries(versions).filter(([, version]) => version !== packageVersion);
if (!cargoVersion || mismatched.length > 0) {
  console.error("Version mismatch:");
  for (const [file, version] of Object.entries(versions)) {
    console.error(`  ${file}: ${version ?? "missing"}`);
  }
  process.exit(1);
}

console.log(`Versions agree: ${packageVersion}`);
