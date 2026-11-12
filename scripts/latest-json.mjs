import fs from "node:fs";
import path from "node:path";

function arg(name) {
  const index = process.argv.indexOf(`--${name}`);
  return index >= 0 ? process.argv[index + 1] : undefined;
}

function required(name) {
  const value = arg(name);
  if (!value) {
    console.error(`Missing --${name}`);
    process.exit(1);
  }
  return value;
}

function read(file) {
  if (!fs.existsSync(file)) {
    console.error(`Missing file: ${file}`);
    process.exit(1);
  }
  return fs.readFileSync(file, "utf8").trim();
}

const version = required("version").replace(/^v/, "");
const windows = required("windows");
const windowsSig = required("windows-sig");
const linux = required("linux");
const linuxSig = required("linux-sig");
const output = required("output");
const repo = arg("repo") ?? process.env.GITHUB_REPOSITORY ?? "arijumadi/mcp-chat";
const notesFile = arg("notes-file");
const notes = notesFile ? read(notesFile) : "See the GitHub release notes.";
const base = `https://github.com/${repo}/releases/download/v${version}`;

const payload = {
  version,
  notes,
  pub_date: new Date().toISOString(),
  platforms: {
    "windows-x86_64": {
      signature: read(windowsSig),
      url: `${base}/${path.basename(windows)}`,
    },
    "linux-x86_64": {
      signature: read(linuxSig),
      url: `${base}/${path.basename(linux)}`,
    },
  },
};

fs.mkdirSync(path.dirname(output), { recursive: true });
fs.writeFileSync(output, `${JSON.stringify(payload, null, 2)}\n`);
console.log(`Wrote ${output} for ${version}`);
