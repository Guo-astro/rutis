"""{{name}}: a rutis plugin."""

# Guide: https://github.com/arcships/rutis/blob/main/docs/guide/python-plugin.en.md

from rutis import define_plugin


class Greeter:
    def __init__(self, greeting: str) -> None:
        self.greeting = greeting

    async def hello(self, name: str) -> str:
        return f"{self.greeting}, {name}!"


def apply(ctx, config):
    # config has the schema's default when the row leaves greeting out.
    ctx.provide("greeter", Greeter(config["greeting"]))


plugin = define_plugin(
    apply,
    # The services it uses (step 3 of the guide): inject=["translate"], then
    # ctx.use("translate") in apply.
    inject=[],
    # The services it provides, so a host knows them before the plugin runs:
    # the class's public methods.
    provides={"greeter": Greeter},
    # The JSON Schema of its config (step 4).
    config={"type": "object", "properties": {"greeting": {"type": "string", "default": "Hello"}}},
)
