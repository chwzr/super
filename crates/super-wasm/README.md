# @super-sdk/wasm

A browser WebAssembly client for the language-neutral SUPER RPC protocol. This
package controls a native SUPER process. It does not run the agent in the module.
Use `super-agent-sdk-wasm` when the agent loop itself must run in the browser.

Browsers cannot run SUPER's filesystem and shell tools inside their sandbox. Run
the native agent explicitly and connect to it:

```sh
super --mode rpc --rpc-listen 127.0.0.1:9944 --no-session
```

```ts
import init, { SuperClient } from "@super-sdk/wasm"
await init()
const client = await SuperClient.connect("ws://127.0.0.1:9944")
client.onEvent((event) => {
  if (event.type === "message_update" && event.assistantMessageEvent.type === "text_delta") {
    console.log(event.assistantMessageEvent.delta)
  }
})
await client.prompt("List the files here")
```

Build with `wasm-pack build --target web --out-dir pkg`. See `demo/index.html`
for a complete page and the repository's `docs/rpc.md` for the protocol.
