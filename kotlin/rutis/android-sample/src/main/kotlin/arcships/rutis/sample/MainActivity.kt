package arcships.rutis.sample

import android.Manifest
import android.app.Activity
import android.content.pm.PackageManager
import android.os.Build
import android.content.Intent
import android.os.Bundle
import android.text.InputType
import android.view.ViewGroup.LayoutParams.MATCH_PARENT
import android.view.ViewGroup.LayoutParams.WRAP_CONTENT
import android.widget.Button
import android.widget.EditText
import android.widget.LinearLayout
import android.widget.TextView
import arcships.rutis.RutisApp
import arcships.rutis.RutisState
import kotlinx.coroutines.Job
import kotlinx.coroutines.MainScope
import kotlinx.coroutines.cancel
import kotlinx.coroutines.launch

/**
 * The access point, this device's id and the token; Start and Stop; and
 * where the application stands with rutis. Rows of rutis's configuration
 * decide which plugins load.
 *
 * Intent extras fill the fields (`connect`, `id`, `token`) and press a
 * button (`command`: `start` or `stop`), for scripts and tests:
 * `adb shell am start -n arcships.rutis.sample/.MainActivity --es connect
 * ws://127.0.0.1:7443/rutis --es id pixel --es token secret --es command start`.
 */
class MainActivity : Activity() {
    private val scope = MainScope()
    private var watching: Job? = null
    private lateinit var address: EditText
    private lateinit var name: EditText
    private lateinit var token: EditText
    private lateinit var state: TextView

    private val holder get() = application as SampleApplication

    private fun field(hint: String, password: Boolean = false) = EditText(this).apply {
        this.hint = hint
        if (password) inputType = InputType.TYPE_CLASS_TEXT or InputType.TYPE_TEXT_VARIATION_PASSWORD
    }

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        address = field("wss://rutis.example.com:7443/rutis")
        name = field("this device's id: lower-case letters, digits and -")
        token = field("token", password = true)
        state = TextView(this)
        val start = Button(this).apply { text = "Start" }
        val stop = Button(this).apply { text = "Stop" }
        start.setOnClickListener { start() }
        stop.setOnClickListener { holder.rutis?.stop() }
        setContentView(
            LinearLayout(this).apply {
                orientation = LinearLayout.VERTICAL
                setPadding(32, 32, 32, 32)
                for (view in listOf(address, name, token, start, stop, state)) addView(view, MatchWidth)
            },
        )
        holder.rutis?.let(::watch)
        follow(intent)
    }

    override fun onNewIntent(intent: Intent) {
        super.onNewIntent(intent)
        setIntent(intent)
        follow(intent)
    }

    /** What `intent`'s extras say. */
    private fun follow(intent: Intent) {
        intent.getStringExtra("connect")?.let(address::setText)
        intent.getStringExtra("id")?.let(name::setText)
        intent.getStringExtra("token")?.let(token::setText)
        when (intent.getStringExtra("command")) {
            "start" -> start()
            "stop" -> holder.rutis?.stop()
        }
    }

    private fun start() {
        ask()
        val app = holder.rutis ?: try {
            Samples.app(this, address.text.toString().trim(), name.text.toString().trim(), token.text.toString())
        } catch (e: IllegalArgumentException) {
            state.text = e.message
            return
        }.also { holder.rutis = it }
        watch(app)
        app.start()
    }

    /**
     * Asks the user for what the plugins need and is not granted yet. The
     * prompt is this application's: rutis never asks; a plugin without
     * its permission fails, and rutis is told why.
     */
    private fun ask() {
        val missing = PERMISSIONS.filter { checkSelfPermission(it) != PackageManager.PERMISSION_GRANTED }
        if (missing.isNotEmpty()) requestPermissions(missing.toTypedArray(), ASKED)
    }

    /** Granted now: connect again, so that rutis loads the plugins that failed for want of it. */
    override fun onRequestPermissionsResult(requestCode: Int, permissions: Array<out String>, grantResults: IntArray) {
        super.onRequestPermissionsResult(requestCode, permissions, grantResults)
        val app = holder.rutis ?: return
        if (requestCode == ASKED && grantResults.any { it == PackageManager.PERMISSION_GRANTED } && app.state.value !is RutisState.Stopped) {
            app.stop()
            app.start()
        }
    }

    private fun watch(app: RutisApp) {
        watching?.cancel()
        watching = scope.launch { app.state.collect { state.text = it.toString() } }
    }

    override fun onDestroy() {
        scope.cancel()
        super.onDestroy()
    }

    private companion object {
        const val ASKED = 1
        val PERMISSIONS: List<String> = listOfNotNull(
            Manifest.permission.READ_CONTACTS,
            if (Build.VERSION.SDK_INT >= 33) Manifest.permission.POST_NOTIFICATIONS else null,
        )
        val MatchWidth = LinearLayout.LayoutParams(MATCH_PARENT, WRAP_CONTENT)
    }
}
