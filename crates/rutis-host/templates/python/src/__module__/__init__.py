"""{{name}}: a rutis plugin."""

from rutis import define_plugin


class Greeter:
    def __init__(self, greeting: str) -> None:
        self.greeting = greeting

    async def hello(self, name: str) -> str:
        return f"{self.greeting}, {name}!"


def apply(ctx, config):
    # The schema's default is filled in when the row leaves it out.
    ctx.provide("greeter", Greeter(config["greeting"]))
    # Return a cleanup, or use ctx.effect(cleanup), when the plugin holds
    # anything to release.


plugin = define_plugin(
    apply,
    # Services this plugin uses: inject=["llm"], then ctx.use("llm").
    inject=[],
    # What it provides, so that `rutis-host check` (and `check --strict` in a
    # host's CI) knows it before the plugin runs. The class's public methods
    # are the service's: `async def` ones async, the others sync.
    provides={"greeter": Greeter},
    # The JSON Schema of its config: rutis fills its defaults and validates
    # the row's config with it before apply.
    config={"type": "object", "properties": {"greeting": {"type": "string", "default": "Hello"}}},
)
