package arcships.rutis.internal.remote

import arcships.rutis.Plugin
import arcships.rutis.internal.RuntimeSession
import arcships.rutis.internal.peer.EndpointMismatchException
import arcships.rutis.internal.peer.Endpoint
import arcships.rutis.internal.peer.IncompatibleException
import java.util.concurrent.CompletableFuture
import java.util.concurrent.ExecutionException
import java.util.concurrent.TimeUnit
import java.util.concurrent.TimeoutException
import kotlin.math.roundToLong

// Dialing out to a rutis access point, and dialing again: the rules every
// runtime follows (docs/design-remote-runtime-dial-2026-10-14.md §六), as
// rutis-host's `rutis-host/dial` row does (`connect/dial.rs`) and the Go runtime
// (`go/rutis/dial.go`).
//
// A failure is retryable (backoff from 0.5 s, doubling up to 30 s, reset by
// a session that lasted 60 s), an authentication failure (again every 30 s:
// 401 / 403 at the upgrade, a server certificate this side refuses, or the
// access point rejecting this id with close code 4003 after the hello), or
// incompatible (stop: retrying cannot fix it). Every session is new: the
// rows of one go when it ends, and the next connection is mounted afresh.

/** What a failed dial or an ended session means for retrying. */
enum class Failure(val shown: String) {
    Retryable("retryable"),
    AuthRejected("auth-rejected"),
    Incompatible("incompatible"),
}

/** A dial that failed: [failure] says what it means for retrying. */
class DialException(val failure: Failure, val reason: String) : java.io.IOException(reason)

/** WebSocket close codes the rules tell apart. */
object CloseCode {
    const val NORMAL: Int = 1000
    const val GOING_AWAY: Int = 1001
    const val PROTOCOL_ERROR: Int = 1002
    const val UNSUPPORTED: Int = 1003
    const val NO_STATUS: Int = 1005
    const val INVALID_DATA: Int = 1007
    const val TOO_BIG: Int = 1009

    /** A newer connection with the same id took over. */
    const val REPLACED: Int = 4002

    /** An access point rejecting the id a dialer greeted with. */
    const val REJECTED: Int = 4003
}

/**
 * The code a close frame answering the far end's `code` carries: the same,
 * unless a close frame may not carry it (RFC 6455 §7.4.1: 1005 "no status"
 * and 1006 are never sent, nor anything outside the defined ranges), then
 * 1000.
 */
fun echoedCloseCode(code: Int): Int = when (code) {
    in 1000..1003, in 1007..1014, in 3000..4999 -> code
    else -> CloseCode.NORMAL
}

/** A connection the session runs on: what the far end closed it with, once it did. */
interface Connection : arcships.rutis.internal.peer.Channel {
    /** The code of the far end's close frame; 0 until it closed with one. */
    val farClose: Int
}

/** Reconnection: the defaults of `rutis-host/dial`. */
class Retry(
    /** The first wait after a retryable failure; doubled each time. */
    val initialMillis: Long = 500,
    val maxMillis: Long = 30_000,
    /** ± this fraction of each wait, at random. */
    val jitter: Double = 0.2,
    /** A session lasting this long resets the backoff. */
    val stableMillis: Long = 60_000,
    /** The wait after an authentication failure. */
    val rejectedMillis: Long = 30_000,
    /** How long the access point may take to greet. */
    val handshakeMillis: Long = 10_000,
) {
    companion object {
        /**
         * The defaults, unless tests shorten them with `configured`
         * (`RUTIS_RETRY=<initial ms>,<max ms>,<stable ms>,<rejected ms>`).
         */
        fun from(configured: String?): Retry {
            val parts = configured?.split(",") ?: return Retry()
            if (parts.size != 4) return Retry()
            val ms = parts.map { it.trim().toLongOrNull()?.takeIf { n -> n >= 0 } ?: return Retry() }
            return Retry(initialMillis = ms[0], maxMillis = ms[1], stableMillis = ms[2], rejectedMillis = ms[3])
        }
    }
}

/** Exponential backoff with jitter; `random` is in [0, 1). */
class Backoff(private val retry: Retry, private val random: () -> Double = Math::random) {
    private var next = retry.initialMillis

    fun reset() {
        next = retry.initialMillis
    }

    /** The next wait: the current step ± jitter; the step doubles up to the maximum. */
    fun waitMillis(): Long {
        val base = next
        next = minOf(next * 2, retry.maxMillis)
        return (base * (1 + retry.jitter * (2 * random() - 1))).roundToLong()
    }
}

/** The ping interval and how long a silent far end is kept (`RUTIS_HEARTBEAT=<ping ms>,<timeout ms>` for tests). */
class Heartbeat(val pingMillis: Long = 10_000, val timeoutMillis: Long = 30_000) {
    companion object {
        fun from(configured: String?): Heartbeat {
            val parts = configured?.split(",") ?: return Heartbeat()
            val ping = parts.getOrNull(0)?.trim()?.toLongOrNull()
            val timeout = parts.getOrNull(1)?.trim()?.toLongOrNull()
            if (parts.size != 2 || ping == null || timeout == null || ping <= 0 || timeout <= 0) return Heartbeat()
            return Heartbeat(ping, timeout)
        }
    }
}

/** `millis` for a state line: 0.5s, 1s, 30s. */
fun seconds(millis: Long): String {
    val tenths = (millis / 100.0).roundToLong()
    return if (tenths % 10 == 0L) "${tenths / 10}s" else "${tenths / 10}.${tenths % 10}s"
}

/**
 * What a session's end means for retrying: the access point rejecting this
 * id (4003), or the far end greeting as another endpoint than expected, is
 * an authentication failure; a greeting this side cannot speak (an older
 * host refused included) is incompatible; anything else, being replaced
 * (4002) too, is retryable.
 */
fun sessionFailure(error: Throwable?, farClose: Int): Failure = when {
    farClose == CloseCode.REJECTED -> Failure.AuthRejected
    error is EndpointMismatchException -> Failure.AuthRejected
    error is IncompatibleException -> Failure.Incompatible
    else -> Failure.Retryable
}

/** The host of `spec`, an access point to dial: a ws:// or wss:// address; a non-loopback one needs wss://. */
fun dialHost(spec: String): String {
    val uri = try {
        java.net.URI(spec)
    } catch (_: java.net.URISyntaxException) {
        null
    }
    val scheme = uri?.scheme
    val host = uri?.host?.removePrefix("[")?.removeSuffix("]")
    if (uri == null || (scheme != "ws" && scheme != "wss") || host.isNullOrEmpty()) {
        throw IllegalArgumentException("$spec is not a ws:// or wss:// address")
    }
    if (scheme == "ws" && !loopback(host)) throw IllegalArgumentException("$spec: a non-loopback address needs wss://")
    return host
}

/** Whether `host` (a name or an address literal, without brackets) is this machine. */
fun loopback(host: String): Boolean {
    if (host == "localhost") return true
    if (host == "::1" || host == "0:0:0:0:0:0:0:1") return true
    val parts = host.split(".")
    return parts.size == 4 && parts[0] == "127" && parts.all { p -> p.toIntOrNull()?.let { it in 0..255 } == true }
}

/** What happened to a session, as the state of an application sees it. */
sealed class Status {
    /** Dialing, or waiting before dialing again (`retry` says why, after a failure). */
    class Connecting(val retry: String?) : Status()

    /** Greeted: rutis drives the plugins. */
    class Connected(val address: String) : Status()
}

/** How a dialing or listening runtime ended. */
sealed class Ended {
    /** Stopped by this side: whether every cleanup finished in time. */
    class Stopped(val clean: Boolean) : Ended()

    /** The far end is incompatible (retrying cannot fix it): the reason. */
    class Refused(val reason: String) : Ended()
}

/**
 * Dials an access point with [dial] and stays connected: each connection is
 * a new session serving [plugins] (rutis's rows run them, or, given
 * [own], an application installs them once greeted), whose rows go
 * when it ends; it dials
 * again by the rules until [stop] (the rows go, then the connection closes)
 * or the access point is incompatible. [say] gets one state line per change
 * (without the `rutis: ` prefix); [status] sees the same as values.
 */
class Dialing(
    private val address: String,
    private val endpoint: Endpoint,
    private val plugins: List<Plugin>,
    private val retry: Retry,
    private val dial: () -> Connection,
    private val say: (String) -> Unit,
    private val status: (Status) -> Unit = {},
    private val shutdownMillis: Long? = null,
    /** An application's plugins with their configurations, which it installs itself once greeted. */
    private val own: List<Pair<Plugin, Any?>>? = null,
) {
    private val stopping = CompletableFuture<Unit>()

    /** Stops: [run] returns once the rows went and the connection closed. */
    fun stop() {
        stopping.complete(Unit)
    }

    /** Waits for `future` or [stop], at most `millis` (forever when null): whether `future` completed. */
    private fun await(future: CompletableFuture<*>, millis: Long? = null): Boolean {
        try {
            val either = CompletableFuture.anyOf(future, stopping)
            if (millis == null) either.get() else either.get(millis, TimeUnit.MILLISECONDS)
        } catch (_: TimeoutException) {
        } catch (_: ExecutionException) {
        }
        return future.isDone
    }

    /** A connection that failed, as [failure] says. */
    private class Failed(val failure: Failure, val reason: String, val lost: String)

    /** Dials, serves and dials again; returns when stopped or refused. */
    fun run(): Ended {
        val backoff = Backoff(retry)
        status(Status.Connecting(null))
        while (true) {
            val failed = when (val outcome = once(backoff)) {
                is Ended -> return outcome
                else -> outcome as Failed
            }
            val reason = failed.reason
            val (wait, line) = when (failed.failure) {
                Failure.Incompatible -> return Ended.Refused(reason)
                Failure.AuthRejected -> retry.rejectedMillis to "refused: auth-rejected ($reason)"
                Failure.Retryable -> backoff.waitMillis() to "${failed.lost} ($reason)"
            }
            say("$line; retrying in ${seconds(wait)}")
            status(Status.Connecting(line))
            try {
                stopping.get(wait, TimeUnit.MILLISECONDS)
                return Ended.Stopped(true)
            } catch (_: TimeoutException) {
            }
        }
    }

    /** Dials once and serves the session until it ends: [Ended] when stopped, else [Failed]. */
    private fun once(backoff: Backoff): Any {
        val attempt = CompletableFuture<Connection>()
        Thread({
            try {
                attempt.complete(dial())
            } catch (e: Throwable) {
                attempt.completeExceptionally(e)
            }
        }, "rutis-dial").apply { isDaemon = true }.start()
        if (!await(attempt) || stopping.isDone) {
            // A dial that connects after the runtime stopped is closed.
            attempt.thenAccept { it.close("the runtime stopped") }
            return Ended.Stopped(true)
        }
        val connection = try {
            attempt.get()
        } catch (e: ExecutionException) {
            val cause = e.cause ?: e
            if (cause is DialException) return Failed(cause.failure, cause.reason, "cannot connect")
            return Failed(Failure.Retryable, cause.message ?: cause.toString(), "cannot connect")
        }
        val session = try {
            RuntimeSession.start(connection, endpoint, plugins, shutdownMillis, own)
        } catch (e: Exception) {
            connection.close("the session did not start")
            return Failed(Failure.Retryable, "the connection did not start: ${e.message}", "cannot connect")
        }
        val peer = session.peer
        if (!await(peer.readyFuture, retry.handshakeMillis)) {
            if (stopping.isDone) return Ended.Stopped(session.stop("the runtime stopped"))
            peer.shutdown("the access point did not greet in time")
        } else if (peer.error == null) {
            say("connected to $address as ${endpoint.local}")
            status(Status.Connected(address))
            session.install()
            val since = System.nanoTime()
            if (!await(peer.closedFuture)) return Ended.Stopped(session.stop("the runtime stopped"))
            if ((System.nanoTime() - since) / 1_000_000 >= retry.stableMillis) backoff.reset()
        }
        val ended = peer.error
        val failure = sessionFailure(ended, connection.farClose)
        session.end()
        connection.close("the session ended")
        return Failed(failure, ended?.message ?: "the host disconnected", "connection lost")
    }
}
