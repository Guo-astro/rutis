package arcships.rutis.sample

import android.app.Application
import arcships.rutis.RutisApp

/**
 * Holds the connection to rutis for as long as the process lives, not as
 * long as a screen: when to start and stop it is this application's
 * business (here, the buttons of [MainActivity]).
 */
class SampleApplication : Application() {
    var rutis: RutisApp? = null
}
