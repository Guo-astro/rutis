package arcships.rutis.sample.plugins

import android.content.Context
import android.content.pm.PackageManager

/**
 * A permission the user has not granted this application. It crosses to
 * whoever called with its `code` and `data` (design §六): a rutis row sees
 * `permission-denied` and which permission.
 */
class PermissionDenied(val permission: String) :
    Exception("$permission is not granted to this application: grant it in the application (its settings, or when it asks)") {
    val code: String = "permission-denied"
    val data: Map<String, String> = mapOf("permission" to permission)
}

/** Throws [PermissionDenied] unless `permission` is granted. The application asks for it; a plugin only checks. */
fun Context.require(permission: String) {
    if (checkSelfPermission(permission) != PackageManager.PERMISSION_GRANTED) throw PermissionDenied(permission)
}
