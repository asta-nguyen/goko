// Commits since the last release: each with its GitHub login, PR number and
// whether CHANGELOG.md already names its hash.
//
//   node commits.mjs             every commit, oldest first
//   node commits.mjs --missing   only the user-visible ones the changelog lacks;
//                                exits 1 if there are any
import { execFileSync } from "node:child_process";
import fs from "node:fs";

const run = (cmd, args) =>
  execFileSync(cmd, args, { encoding: "utf8", stdio: ["ignore", "pipe", "ignore"] }).trim();
const tryRun = (cmd, args) => {
  try {
    return run(cmd, args);
  } catch {
    return "";
  }
};

const onlyMissing = process.argv.includes("--missing");
const root = run("git", ["rev-parse", "--show-toplevel"]);
const changelog = fs.readFileSync(`${root}/CHANGELOG.md`, "utf8");
// The last release is the newest `release: [x.y.z] - date` commit.
const base = run("git", ["log", "-1", "--format=%H", "--grep=^release: \\["]);
if (!base) {
  console.error("no `release: [x.y.z]` commit found");
  process.exit(2);
}

// Not user-visible on their own: a PR's merge commit (its commits are listed),
// tests, docs, CI, chores, builds, style and the release itself.
const SKIP = /^(test|docs|ci|chore|build|style|release)(\(.+\))?!?:|^Merge |^Release \(#\d+\)$/;

// sha -> PR number, from merge commits ("Merge pull request #N") and from
// squash merges ("title (#N)").
const pr = new Map();
const log = run("git", ["log", "--format=%H%x1f%P%x1f%s", `${base}..HEAD`]);
for (const row of log.split("\n").filter(Boolean)) {
  const [sha, parents, subject] = row.split("\x1f");
  const merged = subject.match(/^Merge pull request #(\d+)/);
  const [first, second] = parents.split(" ");
  if (merged && second) {
    for (const c of run("git", ["rev-list", `${first}..${second}`]).split("\n")) pr.set(c, merged[1]);
  }
  const squashed = subject.match(/\(#(\d+)\)$/);
  if (squashed) pr.set(sha, squashed[1]);
}

// The GitHub login of an author. A noreply address and the commits API are
// exact; an author name is only a guess, so it comes back with a `?` to confirm.
function login(sha, name, email) {
  const noreply = email.match(/^(?:\d+\+)?([^@]+)@users\.noreply\.github\.com$/);
  if (noreply) return noreply[1];
  const api = tryRun("gh", ["api", `repos/{owner}/{repo}/commits/${sha}`, "--jq", ".author.login // empty"]);
  return api || `${name}?`;
}

const rows = run("git", [
  "log",
  "--reverse",
  "--format=%H%x1f%an%x1f%ae%x1f%s%x1f%(trailers:key=Co-authored-by,valueonly,separator=%x3B )",
  `${base}..HEAD`,
]);
let missing = 0;
for (const row of rows.split("\n").filter(Boolean)) {
  const [sha, name, email, subject, co] = row.split("\x1f");
  const short = sha.slice(0, 7);
  const status = SKIP.test(subject) ? "skip" : changelog.includes(short) ? "ok" : "MISSING";
  if (status === "MISSING") missing += 1;
  if (onlyMissing && status !== "MISSING") continue;
  const who = login(sha, name, email) + (co.trim() ? `  +co: ${co.trim()}` : "");
  const n = pr.get(sha);
  console.log(`${status.padEnd(7)} ${short} ${who.padEnd(14)} ${n ? `#${n}` : "-"}\t${subject}`);
}
process.exit(onlyMissing && missing ? 1 : 0);
