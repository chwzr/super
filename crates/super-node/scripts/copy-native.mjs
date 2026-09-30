import { copyFileSync, existsSync, mkdirSync } from "node:fs"
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
    const destination = join(
      "native",
      process.env.SUPER_NODE_PLATFORM ?? `${process.platform}-${process.arch}`,
    )
    mkdirSync(destination, { recursive: true })
    copyFileSync(source, join(destination, "super.node"))
    process.exit(0)
  }
}
throw new Error(`Could not find the native library for ${process.platform}/${process.arch}`)
