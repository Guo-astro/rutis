// A runtime that answers `mount` with none of the features rows need, as a
// runtime that does not know the row contract would.
import { Client } from '../../src/client.ts'

// Started as a host starts a runtime: `<channel> --id <name> <project>`.
const [channel, , id] = process.argv.slice(2)
const client = await Client.connect(channel, {
  endpoint: { local: id, declare: ['runtime'] },
  dispatch: (target, method) => {
    if (target === '' && method === 'mount') return { services: {}, features: [] }
    throw new Error(`no ${method}`)
  },
})
await client.closed()
process.exit(0)
