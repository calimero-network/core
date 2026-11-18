package com.calimero.mero.sample

import org.junit.Assert.assertTrue
import org.junit.Test
import java.io.File

/**
 * The app's preferences (the node URL, the encrypted session) leave the device by no route:
 * `allowBackup="false"` stops cloud backup, but from Android 12 device-to-device transfer ignores
 * it and only the data extraction rules exclude them.
 */
class BackupRulesTest {
    private fun read(path: String) = File(path).readText()

    @Test
    fun `nothing is backed up or transferred`() {
        val manifest = read("src/main/AndroidManifest.xml")
        assertTrue(manifest.contains("android:allowBackup=\"false\""))
        assertTrue(manifest.contains("android:dataExtractionRules=\"@xml/data_extraction_rules\""))
        val rules = read("src/main/res/xml/data_extraction_rules.xml").replace(Regex("<!--.*?-->", RegexOption.DOT_MATCHES_ALL), "")
        for (section in listOf("cloud-backup", "device-transfer")) {
            val body =
                Regex("<$section>(.*?)</$section>", RegexOption.DOT_MATCHES_ALL)
                    .find(rules)
                    ?.groupValues
                    ?.get(1)
                    .orEmpty()
            for (domain in listOf("root", "file", "database", "sharedpref", "external")) {
                assertTrue("$section excludes $domain", body.contains("<exclude domain=\"$domain\" path=\".\" />"))
            }
        }
    }
}
