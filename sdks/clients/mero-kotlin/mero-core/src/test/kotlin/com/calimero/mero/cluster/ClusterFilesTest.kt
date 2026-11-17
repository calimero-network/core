package com.calimero.mero.cluster

import com.calimero.mero.FILE_PART_BYTES
import com.calimero.mero.FileSink
import com.calimero.mero.NotFound
import com.calimero.mero.crypto.Hex
import com.calimero.mero.crypto.sha256
import kotlinx.coroutines.runBlocking
import org.junit.Assert.assertArrayEquals
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.BeforeClass
import org.junit.Test
import java.security.MessageDigest
import java.security.SecureRandom

/** Sealed files and replica attestation against a real local merod 1.0 cluster ([Cluster]): mero-js's `files-pipelines.test.ts`. */
class ClusterFilesTest {
    companion object {
        @BeforeClass
        @JvmStatic
        fun cluster() = Cluster.require()
    }

    private fun random(size: Int) = ByteArray(size).also { SecureRandom().nextBytes(it) }

    @Test
    fun uploadsAMultiPartFileReadsItsInfoAndExportsItBackByteForByte() =
        runBlocking<Unit> {
            val (alice) = Cluster.person(0, "alice-files")
            val app = Cluster.deployKv(alice, "sealed")
            val data = random(FILE_PART_BYTES + 3_000_000)
            val up = alice.uploadFile(app, data)
            assertEquals(data.size.toLong(), up.size)
            assertEquals(Hex.encode(sha256(data)), up.sha256)

            val info = alice.apps.fileInfo(app = app, id = up.id)
            assertEquals(up.id, info.id)
            assertEquals(data.size.toLong(), info.propertySize)
            assertEquals(up.sha256, info.sha256)

            val back = alice.exportFile(app, up.id)
            assertArrayEquals(data, back)

            val digest = MessageDigest.getInstance("SHA-256")
            var written = 0L
            val exported =
                alice.exportFileTo(app, up.id) {
                    digest.update(it)
                    written += it.size
                }
            assertEquals(up.sha256, exported.sha256)
            assertEquals(up.size, written)
            assertEquals(up.sha256, Hex.encode(digest.digest()))
        }

    @Test
    fun uploadsASmallFileInOneRequestAndExportsItFromAnotherNode() =
        runBlocking<Unit> {
            val (alice, pk) = Cluster.person(0, "alice-small")
            val app = Cluster.deployKv(alice, "small-files")
            val data = random(50_000)
            val up = alice.uploadFile(app, data)
            val elsewhere = Cluster.client(1).also { it.signIn(pk) }
            assertArrayEquals(data, elsewhere.exportFile(app, up.id))
            val empty = alice.uploadFile(app, ByteArray(0))
            val out = mutableListOf<ByteArray>()
            alice.exportFileTo(app, empty.id, FileSink { out += it })
            assertTrue(out.isEmpty())
        }

    @Test
    fun refusesAnExportToAStranger() =
        runBlocking<Unit> {
            val (alice) = Cluster.person(0, "alice-owner")
            val (mallory) = Cluster.person(1, "mallory")
            val app = Cluster.deployKv(alice, "private-files")
            val up = alice.uploadFile(app, random(50_000))
            refused<NotFound> { mallory.exportFile(app, up.id) }
            refused<NotFound> { mallory.apps.fileInfo(app = app, id = up.id) }
        }

    @Test
    fun verifiesEveryReplicaOfAnApp() =
        runBlocking<Unit> {
            val (alice) = Cluster.person(0, "alice-replicas")
            val app = Cluster.deployKv(alice, "attested")
            val attested = alice.verifyAppCluster(app)
            assertTrue("replicas: ${attested.keys.keys}", attested.keys.isNotEmpty())
            assertTrue(attested.cluster.isNotEmpty())
            attested.keys.values.forEach { assertEquals(32, it.size) }
            // A stranger may not see the app's replicas; a run the app never had has no receipt.
            val (bob) = Cluster.person(2, "bob-replicas")
            refused<NotFound> { bob.verifyAppCluster(app) }
            val e = refused<com.calimero.mero.CalimeroError> { alice.pipelineReceipt(app, "no-such-run") }
            assertEquals(404, e.status)
        }
}
