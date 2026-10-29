from rutis import define_plugin


class FakeTranslate:
    async def text(self, text: str) -> str:
        return {"Hello": "Bonjour", "Hi": "Salut"}.get(text, text)


def apply(ctx, config):
    ctx.provide("translate", FakeTranslate())


plugin = define_plugin(apply, provides={"translate": FakeTranslate})
