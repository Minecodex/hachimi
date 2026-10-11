import { existsSync, realpathSync, statSync } from "node:fs";
import { delimiter, isAbsolute, join } from "node:path";

// Cargo test executables run outside the Desktop's runtime manager. Supply the
// existing test-only override without adding ambient PATH fallback to Workers.
export function resolveTestGit(environment = process.env, platform = process.platform) {
  const explicit = environment.HACHIMI_GIT_EXECUTABLE;
  if (explicit) {
    if (!isAbsolute(explicit) || !existsSync(explicit) || !statSync(explicit).isFile()) {
      throw new Error("The test Git override must name an existing absolute executable");
    }
    return realpathSync(explicit);
  }
  const executable = platform === "win32" ? "git.exe" : "git";
  for (const directory of (environment.PATH ?? "").split(delimiter)) {
    if (!isAbsolute(directory)) continue;
    const candidate = join(directory, executable);
    if (existsSync(candidate) && statSync(candidate).isFile()) return realpathSync(candidate);
  }
  throw new Error("Cargo Git tests require system Git on an absolute PATH entry");
}
