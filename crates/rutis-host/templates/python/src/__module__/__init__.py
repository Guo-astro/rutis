"""{{name}}: a rutis plugin."""

from rutis import define_plugin


class Greeter:
    def __init__(self, greeting: str) -> None:
        self.greeting = greeting

    def hello(self, name: str) -> str:
        return f"{self.greeting}, {name}!"


def apply(ctx, config):
    ctx.provide("greeter", Greeter(config.get("greeting", "Hello")))
    # Return a cleanup, or use ctx.effect(cleanup), when the plugin holds
    # anything to release.


plugin = define_plugin(
    apply,
    # Services this plugin uses: inject=["llm"], then ctx.use("llm").
    inject=[],
    # What ctx.provide provides reaches rutis by itself, every method async.
    # Declare provides={"greeter": Greeter} only to make methods sync (those
    # not `async def`), or for `rutis-host check` to know it before the
    # plugin runs.
    # The JSON Schema of its config.
    config={"type": "object", "properties": {"greeting": {"type": "string", "default": "Hello"}}},
)
