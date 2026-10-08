import { createConnection } from 'node:net'
import { ConnectError } from './errors.mjs'
import { frame } from './unix.mjs'

// The environment variable holding the token a process started on a
// loopback channel presents first (see rutis-bridge's `Handover::Loopback`).
export const TOKEN = 'RUTIS_CHANNEL_TOKEN'

// Dial `tcp:<host>:<port>` (a loopback address the starting process listens
// on), send the token from the environment and a newline, then frame lines.
export async function open(spec, handlers) {
  const address = spec.slice('tcp:'.length)
  const at = address.lastIndexOf(':')
  const host = address.slice(0, at).replace(/^\[(.*)\]$/, '$1')
  const port = Number(address.slice(at + 1))
  if (at < 0 || !Number.isInteger(port)) throw new ConnectError('incompatible', `not a tcp address: ${spec}`)
  const token = process.env[TOKEN]
  if (!token) throw new ConnectError('auth-rejected', `${TOKEN} is not set for ${spec}`)
  const stream = createConnection({ host, port })
  try {
    await new Promise((resolve, reject) => { stream.once('connect', resolve); stream.once('error', reject) })
  } catch (error) {
    stream.destroy()
    throw new ConnectError('retryable', `${address}: ${error.message}`)
  }
  stream.setNoDelay(true)
  stream.write(token + '\n')
  return frame(stream, handlers)
}
