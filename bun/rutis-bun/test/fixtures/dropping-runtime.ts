// The Bun runtime, made to drop the notifications that withdraw a service:
// the latest a withdrawal can reach rutis is never.
import { Client } from '../../src/client.ts'
import { Runtime } from '../../src/runner.ts'

// Started as a host starts a runtime: `<channel> --id <name> <project>`.
const [channel, , id, project] = process.argv.slice(2)
const runtime = new Runtime(project)
const client = await Client.connect(channel, { dispatch: runtime.dispatch, endpoint: { local: id, declare: ['runtime'] } })
const callAsync = client.callAsync.bind(client)
client.callAsync = (target: string, method: string, args: any) =>
  method === 'service' && args[1] === null ? Promise.resolve(null) : callAsync(target, method, args)
runtime.client = client
await client.closed()
process.exit(0)
