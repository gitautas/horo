import fs from "node:fs";
import { execFileSync } from "node:child_process";

const type = process.argv[2];

if (!["patch", "minor", "major"].includes(type)) {
  console.error("Usage: npm run release:{patch|minor|major}");
  process.exit(1);
}

function run(command, args) {
  console.log(`$ ${command} ${args.join(" ")}`);
  execFileSync(command, args, { stdio: "inherit" });
}

// Captures stdout instead of inheriting it, for the preflight checks.
function capture(command, args) {
  return execFileSync(command, args, { encoding: "utf8" }).trim();
}

function fail(message) {
  console.error(`\nAborting: ${message}`);
  process.exit(1);
}

// Everything below runs before a single file is touched, so a failed check
// never leaves behind a half-made release to unpick by hand.

// The push target is whatever the current branch already tracks, so this works
// in a fork (origin) and in the source repo alike without hardcoding a name.
let remote;
try {
  remote = capture("git", ["rev-parse", "--abbrev-ref", "--symbolic-full-name", "@{u}"]).split("/")[0];
} catch {
  fail("current branch has no upstream. Push it once with `git push -u origin HEAD`.");
}

if (capture("git", ["status", "--porcelain"])) {
  fail("working tree is dirty. Commit or stash first — a release commit should only contain the version bump.");
}

const branch = capture("git", ["rev-parse", "--abbrev-ref", "HEAD"]);

// A tag on a stale branch builds code that isn't what's on the remote.
run("git", ["fetch", remote, branch]);

if (capture("git", ["rev-parse", "HEAD"]) !== capture("git", ["rev-parse", `${remote}/${branch}`])) {
  fail(`${branch} is out of sync with ${remote}/${branch}. Pull or push first.`);
}

function getVersion() {
  const cargo = fs.readFileSync("Cargo.toml", "utf8");

  const match = cargo.match(/^version\s*=\s*"([^"]+)"/m);

  if (!match) {
    throw new Error("Could not find version in Cargo.toml");
  }

  return match[1];
}

function bumpVersion(version, type) {
  const [major, minor, patch] = version.split(".").map(Number);

  if (type === "major") return `${major + 1}.0.0`;
  if (type === "minor") return `${major}.${minor + 1}.0`;
  return `${major}.${minor}.${patch + 1}`;
}

const currentVersion = getVersion();
const newVersion = bumpVersion(currentVersion, type);
const tag = `v${newVersion}`;

// Re-tagging a version that already shipped would collide with its GitHub release.
const existingTags = capture("git", ["tag", "--list", tag]);

if (existingTags) {
  fail(`tag ${tag} already exists locally.`);
}

console.log(`Bumping ${currentVersion} → ${newVersion}`);

// Update Cargo.toml — the workspace root, whose `[workspace.package]` version
// every crate inherits.
const cargoPath = "Cargo.toml";
let cargo = fs.readFileSync(cargoPath, "utf8");

cargo = cargo.replace(
  /^version\s*=\s*"[^"]+"/m,
  `version = "${newVersion}"`
);

fs.writeFileSync(cargoPath, cargo);

// Cargo.lock records the workspace member's own version, so it needs the bump
// too — otherwise the next cargo run dirties the tree right after a release,
// and the tagged tree has a lockfile that disagrees with Cargo.toml.
// --workspace re-resolves only the members, leaving dependency versions alone.
run("cargo", [
  "update",
  "--workspace",
  "--offline",
  "--manifest-path",
  cargoPath,
]);

// Update package.json
const packagePath = "package.json";
const pkg = JSON.parse(fs.readFileSync(packagePath, "utf8"));

pkg.version = newVersion;

fs.writeFileSync(
  packagePath,
  JSON.stringify(pkg, null, 2) + "\n"
);

// Update package-lock.json if you have one
if (fs.existsSync("package-lock.json")) {
  run("npm", ["install", "--package-lock-only", "--ignore-scripts"]);
}

// Make sure everything is clean before committing
run("git", ["add", "package.json", "Cargo.toml", "Cargo.lock"]);

if (fs.existsSync("package-lock.json")) {
  run("git", ["add", "package-lock.json"]);
}

run("git", ["commit", "-m", `chore: release ${tag}`]);

// Create tag
run("git", ["tag", tag]);

// The commit goes first: pushing the tag is what starts the release workflow,
// and it checks out the tag, so the bump must already be on the remote.
run("git", ["push", remote, "HEAD"]);
run("git", ["push", remote, tag]);

const repo = capture("git", ["remote", "get-url", remote])
  .replace(/^git@github\.com:/, "")
  .replace(/^https:\/\/github\.com\//, "")
  .replace(/\.git$/, "");

console.log(`
Pushed ${tag}. The Release workflow is now building all five platforms
(~30-90 min). It publishes the release automatically once every platform
succeeds; until then it stays a draft.

  https://github.com/${repo}/actions/workflows/release.yml
`);
