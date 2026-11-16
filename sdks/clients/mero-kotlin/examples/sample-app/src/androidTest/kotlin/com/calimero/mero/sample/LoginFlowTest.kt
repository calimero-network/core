package com.calimero.mero.sample

import android.content.Intent
import androidx.compose.ui.test.assertIsDisplayed
import androidx.compose.ui.test.junit4.createEmptyComposeRule
import androidx.compose.ui.test.onAllNodesWithTag
import androidx.compose.ui.test.onNodeWithTag
import androidx.compose.ui.test.performClick
import androidx.compose.ui.test.performScrollTo
import androidx.test.core.app.ActivityScenario
import androidx.test.core.app.ApplicationProvider
import androidx.test.ext.junit.runners.AndroidJUnit4
import org.junit.Rule
import org.junit.Test
import org.junit.runner.RunWith

/**
 * The CI UI test, with no network: the sign-in screen's two paths (Cloud passkey, developer node),
 * then the in-app demo (mero-testkit's in-memory backend) through Home, Chat and sign-out.
 */
@RunWith(AndroidJUnit4::class)
class LoginFlowTest {
    @get:Rule
    val composeRule = createEmptyComposeRule()

    private fun launch(): ActivityScenario<MainActivity> =
        ActivityScenario.launch(Intent(ApplicationProvider.getApplicationContext(), MainActivity::class.java))

    private fun waitForTag(
        tag: String,
        timeoutMs: Long = 15_000,
    ) {
        composeRule.waitUntil(timeoutMs) { composeRule.onAllNodesWithTag(tag).fetchSemanticsNodes().isNotEmpty() }
    }

    @Test
    fun signInScreenOffersCloudAndDeveloperNode() {
        launch()
        waitForTag("loginTitle")
        composeRule.onNodeWithTag("cloudLoginButton").assertIsDisplayed()
        composeRule.onNodeWithTag("manualNode").performScrollTo().performClick()
        waitForTag("nodeUrlField")
        composeRule.onNodeWithTag("allowMock").performScrollTo().assertIsDisplayed()
        composeRule.onNodeWithTag("useCloud").performScrollTo().performClick()
        waitForTag("cloudUrlField")
    }

    @Test
    fun demoOpensChatAndSignsOut() {
        launch()
        waitForTag("demoButton")
        composeRule.onNodeWithTag("demoButton").performScrollTo().performClick()

        waitForTag("homeTitle")
        composeRule.onNodeWithTag("homeAccount").assertIsDisplayed()
        composeRule.onNodeWithTag("openChat").performScrollTo().performClick()
        waitForTag("chatAdd")

        composeRule.onNodeWithTag("tabHome").performClick()
        waitForTag("signOutButton")
        composeRule.onNodeWithTag("signOutButton").performScrollTo().performClick()
        waitForTag("loginTitle")
    }
}
