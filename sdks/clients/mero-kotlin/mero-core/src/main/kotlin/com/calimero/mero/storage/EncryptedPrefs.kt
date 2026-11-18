package com.calimero.mero.storage

import android.content.Context
import android.content.SharedPreferences
import androidx.security.crypto.EncryptedSharedPreferences
import androidx.security.crypto.MasterKey

/**
 * Opens the [EncryptedSharedPreferences] files the SDK keeps ([EncryptedPrefsTokenStore],
 * [com.calimero.mero.account.EncryptedPrefsSecureStore]), surviving a keyset that no longer
 * decrypts.
 *
 * The file is backed up under `allowBackup` (and moved by device-to-device transfer), but the
 * Keystore master key that wraps its keyset never leaves the device. After a restore,
 * [EncryptedSharedPreferences.create] throws (`AEADBadTagException`, `KeyStoreException`,
 * `InvalidProtocolBufferException`, …): nothing in the file can be read again on this device.
 * Rather than crash the app at start, the file is deleted and opened afresh, so the person
 * signs in again. Exclude the `mero_*` files from backups to avoid the round trip (see the README).
 */
internal object EncryptedPrefs {
    fun open(
        context: Context,
        fileName: String,
    ): SharedPreferences {
        val appContext = context.applicationContext
        return openOrReset(
            open = { create(appContext, fileName) },
            reset = { appContext.deleteSharedPreferences(fileName) },
        )
    }

    /**
     * [open], or — when it throws, as a keyset that does not decrypt does — [reset] and [open]
     * once more. A second failure is not a keyset problem and is thrown.
     */
    @Suppress("TooGenericExceptionCaught")
    internal fun <T> openOrReset(
        open: () -> T,
        reset: () -> Unit,
    ): T =
        try {
            open()
        } catch (_: Exception) {
            reset()
            open()
        }

    private fun create(
        context: Context,
        fileName: String,
    ): SharedPreferences {
        val masterKey =
            MasterKey
                .Builder(context)
                .setKeyScheme(MasterKey.KeyScheme.AES256_GCM)
                .build()
        return EncryptedSharedPreferences.create(
            context,
            fileName,
            masterKey,
            EncryptedSharedPreferences.PrefKeyEncryptionScheme.AES256_SIV,
            EncryptedSharedPreferences.PrefValueEncryptionScheme.AES256_GCM,
        )
    }
}
