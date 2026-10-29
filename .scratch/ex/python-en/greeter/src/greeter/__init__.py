from rutis import define_plugin


class Greeter:
    def __init__(self, ctx, greeting: str) -> None:
        self.ctx = ctx
        self.translate = ctx.use("translate")
        self.greeting = greeting
        self.greeted = 0

    async def hello(self, name: str) -> str:
        word = await self.translate.text(self.greeting)
        self.ctx.emit("greeter/greeted", name)
        return f"{word}, {name}!"

    async def count(self) -> int:
        return self.greeted


def apply(ctx, config):
    greeter = Greeter(ctx, config["greeting"])

    def greeted(name):
        greeter.greeted += 1

    ctx.on("greeter/greeted", greeted)
    ctx.provide("greeter", greeter)


plugin = define_plugin(
    apply,
    inject=["translate"],
    provides={"greeter": Greeter},
    config={"type": "object", "properties": {"greeting": {"type": "string", "default": "Hello"}}},
)
