import { copyFileSync, existsSync } from "node:fs"
import { join } from "node:path"

const names =
  process.platform === "win32"
    ? ["super_node.dll"]
    : process.platform === "darwin"
      ? ["libsuper_node.dylib"]
      : ["libsuper_node.so"]
for (const name of names) {
  const source = join("target", "release", name)
  if (existsSync(source)) {
    copyFileSync(source, "super.node")
    process.exit(0)
  }
}
throw new Error(`Could not find the native library for ${process.platform}/${process.arch}`)
