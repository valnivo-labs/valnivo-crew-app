#!/usr/bin/env node
// Build the Tauri updater manifest from a directory of signed artifacts.
// A universal Mac build is one .app.tar.gz, published for both darwin-aarch64 and darwin-x86_64.
// Windows is the NSIS setup exe (windows-x86_64). The signature is the text of the .sig file.
import { readdirSync, readFileSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { pathToFileURL } from "node:url";

function arg(name) {
  const i = process.argv.indexOf(name);
  if (i < 0 || !process.argv[i + 1]) {
    console.error("missing " + name);
    process.exit(2);
  }
  return process.argv[i + 1];
}

export function buildLatest(o) {
  const files = readdirSync(o.dir);
  const sigOf = (name) => readFileSync(join(o.dir, name + ".sig"), "utf8").trim();
  const urlOf = (name) => "https://github.com/" + o.repo + "/releases/download/" + o.tag + "/" + encodeURIComponent(name);
  const platforms = {};
  // A copied installer with no sibling .sig is a manual-download name, not an update artifact.
  const signed = (name) => files.includes(name + ".sig");
  const tar = files.find((f) => f.endsWith(".app.tar.gz") && signed(f));
  if (tar) {
    const entry = { signature: sigOf(tar), url: urlOf(tar) };
    platforms["darwin-aarch64"] = entry;
    platforms["darwin-x86_64"] = entry;
  }
  const exes = files.filter((f) => f.endsWith(".exe") && signed(f));
  const exe = exes.find((f) => /setup/i.test(f)) || exes[0];
  if (exe) platforms["windows-x86_64"] = { signature: sigOf(exe), url: urlOf(exe) };
  if (!platforms["darwin-aarch64"] || !platforms["windows-x86_64"]) {
    throw new Error("latest.json needs a signed macOS .app.tar.gz and a signed Windows setup exe");
  }
  return { version: o.version, notes: o.notes || "", pub_date: o.pubDate, platforms };
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  const dir = arg("--dir");
  const version = arg("--version");
  const tag = arg("--tag");
  const repo = process.argv.includes("--repo") ? arg("--repo") : "valnivo-labs/valnivo-crew-app";
  const notes = process.argv.includes("--notes") ? arg("--notes") : "";
  const pubDate = process.argv.includes("--date") ? arg("--date") : new Date().toISOString();
  const out = arg("--out");
  const body = buildLatest({ dir, version, tag, repo, notes, pubDate });
  writeFileSync(out, JSON.stringify(body, null, 2) + "\n");
  console.log("wrote " + out);
}
