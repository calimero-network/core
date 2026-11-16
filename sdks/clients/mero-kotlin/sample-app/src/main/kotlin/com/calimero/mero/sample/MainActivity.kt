package com.calimero.mero.sample

import android.content.Intent
import android.os.Bundle
import androidx.activity.ComponentActivity
import androidx.activity.compose.setContent
import androidx.activity.enableEdgeToEdge
import androidx.activity.viewModels
import com.calimero.mero.sample.app.SampleViewModel
import com.calimero.mero.sample.explorer.ExplorerApp
import com.calimero.mero.sample.explorer.ExplorerOptions
import com.calimero.mero.sample.mock.MockBackend

/**
 * Entry point. Sign-in is a passkey on a Calimero Cloud cluster, or "Developer node" for
 * `merod local` (mock attestation allowed, a software passkey), or (debug builds) the in-app demo
 * on mero-testkit's in-memory backend.
 *
 * Two kinds of link come back to this activity (see the manifest): the node's consent page
 * returning to `mero-sample://consent?request_id=…&result=…`, and mero-chat invite links. Both
 * go to [SampleViewModel.handleLink].
 *
 * The launch extras (`demo`, `chatUser`, `invite`, `nodeUrl`) are test and development hooks,
 * read only in debug builds: this activity is the exported launcher, so in a release build any
 * app on the device could set them.
 */
class MainActivity : ComponentActivity() {
    private val vm: SampleViewModel by viewModels()

    private fun testExtra(name: String): String? = if (BuildConfig.DEBUG) intent?.getStringExtra(name) else null

    override fun onCreate(savedInstanceState: Bundle?) {
        enableEdgeToEdge()
        super.onCreate(savedInstanceState)
        val demo = MockBackend.AVAILABLE && BuildConfig.DEBUG && intent?.getBooleanExtra(EXTRA_DEMO, false) == true
        vm.start(demo)
        vm.attach(this)
        if (demo) vm.startDemo()
        val options =
            ExplorerOptions(
                chatDisplayName = testExtra(EXTRA_CHAT_USER),
                autoJoinInvite = testExtra(EXTRA_INVITE),
                nodeUrl = testExtra(EXTRA_NODE_URL),
            )
        consumeLink(intent)
        setContent { ExplorerApp(vm, options) }
    }

    override fun onDestroy() {
        vm.attach(null)
        super.onDestroy()
    }

    override fun onNewIntent(intent: Intent) {
        super.onNewIntent(intent)
        consumeLink(intent)
    }

    private fun consumeLink(intent: Intent?) {
        val data = intent?.data?.toString() ?: return
        vm.handleLink(data)
    }

    companion object {
        const val EXTRA_DEMO = "demo"
        const val EXTRA_CHAT_USER = "chatUser"
        const val EXTRA_INVITE = "invite"

        /** Prefills the developer node URL and opens the sign-in screen on it. */
        const val EXTRA_NODE_URL = "nodeUrl"
    }
}
