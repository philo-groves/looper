# reference-inspector

Reference external plugin for Looper that demonstrates the `plugin_process` actuator path.

## Install In Chat

From the terminal chat view:

`/plugin add looper-agent/external-plugins/reference-inspector`

Then verify:

`/plugin list`

## Actuator

- `text_inspect`: accepts `args.text` and returns summary stats + a sensor output line.

Example action payload (sent by Looper runtime to plugin stdin):

```json
{
  "kind": "actuator_execute",
  "actuator": "text_inspect",
  "args": {
    "text": "hello world"
  },
  "workspace_dir": "C:/projects/my-workspace"
}
```
