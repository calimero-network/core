package com.calimero.mero.tee

import com.calimero.mero.crypto.Hex
import com.calimero.mero.crypto.sha256
import kotlinx.serialization.json.Json
import kotlinx.serialization.json.JsonArray
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive
import kotlinx.serialization.json.intOrNull
import org.bouncycastle.asn1.ASN1Encoding
import org.bouncycastle.asn1.ASN1Integer
import org.bouncycastle.asn1.ASN1Primitive
import org.bouncycastle.asn1.ASN1Sequence
import org.bouncycastle.asn1.x509.BasicConstraints
import org.bouncycastle.asn1.x509.CertificateList
import org.bouncycastle.asn1.x509.Extension
import org.bouncycastle.asn1.x509.Extensions
import org.bouncycastle.asn1.x509.KeyUsage
import org.bouncycastle.asn1.x509.SubjectPublicKeyInfo
import org.bouncycastle.crypto.params.ECPublicKeyParameters
import org.bouncycastle.crypto.util.PublicKeyFactory
import org.bouncycastle.util.encoders.Base64
import java.math.BigInteger
import org.bouncycastle.asn1.x509.Certificate as AsnCertificate

/**
 * DCAP quote verification on the device: a quote's signature chain to Intel's SGX root CA,
 * its QE report, its TCB level against the collateral, and its attributes. Port of `verify`
 * from `@phala/dcap-qvl` 0.5.3 (`verify.js`, `quote.js`, `utils.js`, `tcb_info.js`,
 * `qe_identity.js`), the library mero-js is handed as `dcapVerify`; built in here, as in the
 * Swift SDK (`Dcap.verify`), since there is no such package for Android. The certificate and
 * CRL structures are read with Bouncy Castle's ASN.1 classes, strictly (canonical DER only).
 *
 * Every check dcap-qvl makes is made, in its order, with its messages: CRLs authenticated and
 * current before any revocation answer is used, every certificate in every chain checked
 * against Intel's root and those CRLs, the TCB info and QE identity signed and current, the
 * QE report signed by the PCK key and bound to the attestation key, the quote signed by the
 * attestation key, the platform's TCB level matched and merged with the QE's, `Revoked` (or
 * any unknown status) refused, and a debug or malformed TD refused.
 */
@Suppress("TooManyFunctions")
object Dcap : DcapVerify {
    /** Intel's SGX Root CA, DER (dcap-qvl `TRUSTED_ROOT_CA_DER`). */
    val TRUSTED_ROOT_CA_DER: ByteArray =
        Base64.decode(
            "MIICjzCCAjSgAwIBAgIUImUM1lqdNInzg7SVUr9QGzknBqwwCgYIKoZIzj0EAwIwaDEaMBgGA1UE" +
                "AwwRSW50ZWwgU0dYIFJvb3QgQ0ExGjAYBgNVBAoMEUludGVsIENvcnBvcmF0aW9uMRQwEgYDVQQH" +
                "DAtTYW50YSBDbGFyYTELMAkGA1UECAwCQ0ExCzAJBgNVBAYTAlVTMB4XDTE4MDUyMTEwNDUxMFoX" +
                "DTQ5MTIzMTIzNTk1OVowaDEaMBgGA1UEAwwRSW50ZWwgU0dYIFJvb3QgQ0ExGjAYBgNVBAoMEUlu" +
                "dGVsIENvcnBvcmF0aW9uMRQwEgYDVQQHDAtTYW50YSBDbGFyYTELMAkGA1UECAwCQ0ExCzAJBgNV" +
                "BAYTAlVTMFkwEwYHKoZIzj0CAQYIKoZIzj0DAQcDQgAEC6nEwMDIYZOj/iPWsCzaEKi71OiOSLRF" +
                "hWGjbnBVJfVnkY4u3IjkDYYL0MxO4mqsyYjlBalTVYxFP2sJBK5zlKOBuzCBuDAfBgNVHSMEGDAW" +
                "gBQiZQzWWp00ifODtJVSv1AbOScGrDBSBgNVHR8ESzBJMEegRaBDhkFodHRwczovL2NlcnRpZmlj" +
                "YXRlcy50cnVzdGVkc2VydmljZXMuaW50ZWwuY29tL0ludGVsU0dYUm9vdENBLmRlcjAdBgNVHQ4E" +
                "FgQUImUM1lqdNInzg7SVUr9QGzknBqwwDgYDVR0PAQH/BAQDAgEGMBIGA1UdEwEB/wQIMAYBAf8C" +
                "AQEwCgYIKoZIzj0EAwIDSQAwRgIhAOW/5QkR+S9CiSDcNoowLuPRLsWGf/Yi7GSX94BgwTwgAiEA" +
                "4J0lrHoMs+Xo5o/sX6O9QWxHRAvZUGOdRQ7cvqRXaqI=",
        )

    internal const val TEE_TYPE_SGX = 0x00000000L
    internal const val TEE_TYPE_TDX = 0x00000081L
    private const val ATTESTATION_KEY_TYPE_ECDSA256_WITH_P256_CURVE = 2
    private const val PCK_CERT_CHAIN = 5
    private val ALLOWED_QUOTE_VERSIONS = listOf(3, 4, 5)

    private val json = Json { ignoreUnknownKeys = true }

    /** Verify [quote] against [collateral] as of [nowSecs] with Intel's production root. */
    override fun verify(
        quote: ByteArray,
        collateral: DcapCollateral,
        nowSecs: Long,
    ): DcapVerifiedReport = verify(quote, collateral, nowSecs, TRUSTED_ROOT_CA_DER)

    /**
     * As [verify], against another root CA — for tests. Debug enclaves and service TDs are
     * refused. Throws [TeeTrustException] with dcap-qvl's reason.
     */
    fun verify(
        rawQuote: ByteArray,
        collateral: DcapCollateral,
        nowSecs: Long,
        rootCaDer: ByteArray,
    ): DcapVerifiedReport = guard("DCAP verification failed") { verifyChecked(rawQuote, collateral, nowSecs, rootCaDer) }

    @Suppress("LongMethod", "CyclomaticComplexMethod")
    private fun verifyChecked(
        rawQuote: ByteArray,
        collateral: DcapCollateral,
        nowSecs: Long,
        rootCaDer: ByteArray,
    ): DcapVerifiedReport {
        val quote =
            try {
                ParsedQuote.parse(rawQuote)
            } catch (e: Exception) {
                throw TeeTrustException("Failed to decode quote: ${e.message}", e)
            }
        val header = quote.header
        if (header.version !in ALLOWED_QUOTE_VERSIONS) dcapFail("Unsupported DCAP quote version")
        if (header.teeType != TEE_TYPE_SGX && header.teeType != TEE_TYPE_TDX) dcapFail("Unsupported DCAP TEE type")
        if (header.teeType == TEE_TYPE_SGX && header.version != 3) dcapFail("SGX TEE quote must have version 3")
        if (header.teeType == TEE_TYPE_TDX && header.version != 4 && header.version != 5) dcapFail("TDX TEE quote must have version 4 or 5")
        if (header.attestationKeyType != ATTESTATION_KEY_TYPE_ECDSA256_WITH_P256_CURVE) dcapFail("Unsupported DCAP attestation key type")

        val tcbInfo = guard("Failed to decode TcbInfo") { TcbInfo.parse(json, collateral.tcbInfo) }
        val nowMs = nowSecs * 1000
        if (nowMs < parseIsoMillis(tcbInfo.issueDate)) dcapFail("TCBInfo issue date is in the future")
        if (nowMs > parseIsoMillis(tcbInfo.nextUpdate)) dcapFail("TCBInfo expired")

        val crls = Pki.validateCollateralCrls(rootCaDer, collateral.pckCrlIssuerChain, collateral.rootCaCrl, collateral.pckCrl, nowSecs)

        val tcbLeafCerts = Pki.extractCerts(collateral.tcbInfoIssuerChain)
        if (tcbLeafCerts.size < 2) dcapFail("Certificate chain is too short for TCB Info")
        Pki.verifyCertificateChain(tcbLeafCerts[0], tcbLeafCerts.drop(1), nowSecs, crls, rootCaDer)
        if (!Pki.verifyWithCertificate(tcbLeafCerts[0], collateral.tcbInfo.encodeToByteArray(), collateral.tcbInfoSignature)) {
            dcapFail("Signature is invalid for tcb_info")
        }

        val qeIdentity = guard("Failed to decode QeIdentity") { QeIdentity.parse(json, collateral.qeIdentity) }
        if (nowMs < parseIsoMillis(qeIdentity.issueDate)) dcapFail("QE Identity issue date is in the future")
        if (nowMs > parseIsoMillis(qeIdentity.nextUpdate)) dcapFail("QE Identity expired")
        val qeIdCerts = Pki.extractCerts(collateral.qeIdentityIssuerChain)
        if (qeIdCerts.size < 2) dcapFail("Certificate chain is too short for QE Identity")
        Pki.verifyCertificateChain(qeIdCerts[0], qeIdCerts.drop(1), nowSecs, crls, rootCaDer)
        if (!Pki.verifyWithCertificate(qeIdCerts[0], collateral.qeIdentity.encodeToByteArray(), collateral.qeIdentitySignature)) {
            dcapFail("Signature is invalid for qe_identity")
        }
        val expectedQeId = if (header.teeType == TEE_TYPE_TDX) "TD_QE" else "QE"
        val allowedQeVersions = if (header.teeType == TEE_TYPE_TDX) listOf(2, 3) else listOf(2)
        if (qeIdentity.id != expectedQeId || qeIdentity.version !in allowedQeVersions) {
            dcapFail("Unsupported QE Identity id/version for the quote TEE type")
        }

        val auth = quote.authData
        val certificationData = auth.certificationData
        val pckCerts =
            if (collateral.pckCertificateChain != null) {
                Pki.extractCerts(collateral.pckCertificateChain).ifEmpty { dcapFail("Failed to extract PCK certificates from collateral") }
            } else {
                if (certificationData.certType != PCK_CERT_CHAIN) {
                    dcapFail(
                        "Unsupported DCAP PCK cert format: ${certificationData.certType}. Use get_collateral() to fetch PCK certificate.",
                    )
                }
                Pki.extractCerts(String(certificationData.body, Charsets.UTF_8)).ifEmpty { dcapFail("Failed to extract PCK certificates from quote") }
            }
        if (pckCerts.size < 2) dcapFail("Certificate chain is too short in quote")
        Pki.verifyCertificateChain(pckCerts[0], pckCerts.drop(1), nowSecs, crls, rootCaDer)

        if (!Pki.verifyRawWithCertificate(pckCerts[0], auth.qeReport, auth.qeReportSignature)) {
            dcapFail("Signature is invalid for qe_report in quote")
        }
        val qeReport = QuoteReader(auth.qeReport).enclaveReport()
        val qeHash = sha256(auth.ecdsaAttestationKey + auth.qeAuthData)
        if (!qeHash.contentEquals(qeReport.reportData.copyOfRange(0, 32))) dcapFail("QE report hash mismatch")
        val qeTcbStatus = verifyQeIdentityPolicy(qeReport, qeIdentity)

        val attestationKey = Pki.p256Key(byteArrayOf(0x04) + auth.ecdsaAttestationKey)
        val signed = rawQuote.copyOfRange(0, quote.signedLength)
        if (!verifyRaw64(attestationKey, auth.ecdsaSignature, signed)) dcapFail("ISV enclave report signature is invalid")

        val extension = Pki.intelExtension(pckCerts[0])
        val cpuSvn = Pki.cpuSvn(extension)
        val pceSvn = Pki.pceSvn(extension)
        val fmspc = Pki.fmspc(extension)
        if (!fmspc.contentEquals(guard("Fmspc mismatch") { Hex.decodeUnsized(tcbInfo.fmspc, "fmspc") })) dcapFail("Fmspc mismatch")

        if (header.teeType == TEE_TYPE_TDX && (tcbInfo.version < 3 || tcbInfo.id != "TDX")) {
            dcapFail("TDX quote with non-TDX TCB info in the collateral")
        } else if (header.teeType == TEE_TYPE_SGX && (tcbInfo.version < 2 || tcbInfo.id != "SGX")) {
            dcapFail("SGX quote with non-SGX TCB info in the collateral")
        }

        val platform = matchPlatformTcb(tcbInfo, quote, cpuSvn, pceSvn)
        val final = platform.merge(qeTcbStatus)
        if (!final.isValid) dcapFail("TCB status is invalid: ${final.status}")
        validateAttrs(quote.report)
        return DcapVerifiedReport(final.status, final.advisoryIds, quote.report)
    }

    @Suppress("LoopWithTooManyJumpStatements")
    private fun matchPlatformTcb(
        tcbInfo: TcbInfo,
        quote: ParsedQuote,
        cpuSvn: ByteArray,
        pceSvn: Int,
    ): TcbStatus {
        for (level in tcbInfo.tcbLevels) {
            if (pceSvn < level.pceSvn) continue
            if (level.sgxComponents.isEmpty()) dcapFail("No SGX components in the TCB info")
            if (!svnAtLeast(cpuSvn, level.sgxComponents)) continue
            if (quote.header.teeType == TEE_TYPE_TDX) {
                val td = quote.report.td10 ?: dcapFail("Failed to get TD10 report")
                if (level.tdxComponents.isEmpty()) dcapFail("No TDX components in the TCB info")
                if (!svnAtLeast(td.teeTcbSvn, level.tdxComponents)) continue
            }
            return TcbStatus(level.tcbStatus, level.advisoryIds)
        }
        dcapFail("No matching TCB level found")
    }

    private fun svnAtLeast(
        actual: ByteArray,
        required: List<Int>,
    ): Boolean = actual.size == required.size && actual.indices.all { (actual[it].toInt() and 0xff) >= required[it] }

    private fun verifyQeIdentityPolicy(
        qeReport: SgxEnclaveReport,
        identity: QeIdentity,
    ): TcbStatus {
        val expectedMrSigner = guard("QE MRSIGNER mismatch") { Hex.decodeUnsized(identity.mrsigner, "mrsigner") }
        if (!qeReport.mrSigner.contentEquals(expectedMrSigner)) {
            dcapFail("QE MRSIGNER mismatch: expected ${identity.mrsigner}, got ${Hex.encode(qeReport.mrSigner).uppercase()}")
        }
        if (qeReport.isvProdId != identity.isvprodid) {
            dcapFail("QE ISVPRODID mismatch: expected ${identity.isvprodid}, got ${qeReport.isvProdId}")
        }
        // The QE must always be production-mode; allowing debug only ever governs the workload.
        if (qeReport.attributes[0].toInt() and 0x02 != 0) dcapFail("Debug mode is enabled")

        val miscselect = le32(guard("QE MISCSELECT") { Hex.decodeUnsized(identity.miscselect, "miscselect") })
        val miscselectMask = le32(guard("QE MISCSELECT") { Hex.decodeUnsized(identity.miscselectMask, "miscselectMask") })
        val qeMasked = qeReport.miscSelect and miscselectMask
        val expectedMasked = miscselect and miscselectMask
        if (qeMasked != expectedMasked) {
            dcapFail("QE MISCSELECT mismatch: expected ${hex32(expectedMasked)} (masked), got ${hex32(qeMasked)} (masked)")
        }
        val attributes = guard("QE ATTRIBUTES") { Hex.decodeUnsized(identity.attributes, "attributes") }
        val attributesMask = guard("QE ATTRIBUTES") { Hex.decodeUnsized(identity.attributesMask, "attributesMask") }
        for (i in 0 until 16) {
            val mask = attributesMask.getOrElse(i) { 0 }.toInt() and 0xff
            val expected = (attributes.getOrElse(i) { 0 }.toInt() and 0xff) and mask
            val actual = (qeReport.attributes[i].toInt() and 0xff) and mask
            if (expected != actual) {
                dcapFail("QE ATTRIBUTES mismatch at byte $i: expected ${hex8(expected)} (masked), got ${hex8(actual)} (masked)")
            }
        }
        identity.tcbLevels.firstOrNull { qeReport.isvSvn >= it.isvSvn }?.let { return TcbStatus(it.tcbStatus, it.advisoryIds) }
        if (identity.tcbLevels.isEmpty()) dcapFail("No TCB levels found in QE Identity")
        dcapFail("QE ISVSVN ${qeReport.isvSvn} is below minimum required ${identity.tcbLevels.last().isvSvn} from QE Identity")
    }

    private fun validateAttrs(report: DcapReport) {
        when (report) {
            is DcapReport.Sgx -> if (report.report.attributes[0].toInt() and 0x02 != 0) dcapFail("Debug mode is enabled")
            is DcapReport.Td10 -> validateTd10(report.report)
            is DcapReport.Td15 -> {
                if (report.report.mrServiceTd.any { it.toInt() != 0 }) dcapFail("Invalid mr service td")
                validateTd10(report.report.base)
            }
        }
    }

    private fun validateTd10(report: TdReport10) {
        val a = report.tdAttributes.map { it.toInt() and 0xff }
        val tud = a[0]
        // TUD bit 0 is DEBUG; bits 7:1 are reserved and must always be zero.
        if (tud and 0x01.inv() != 0) dcapFail("Reserved bits in TD attributes are set")
        if (tud and 0x01 != 0) dcapFail("Debug mode is enabled")
        val reservedLower = ((a[3] and 0x0f) shl 16) or (a[2] shl 8) or a[1]
        val septVeDisable = a[3] and 0x10 != 0
        val reservedBit29 = a[3] and 0x20 != 0
        val reservedOther = ((a[7] and 0x7f) shl 24) or (a[6] shl 16) or (a[5] shl 8) or a[4]
        if (reservedLower != 0 || reservedBit29 || reservedOther != 0) dcapFail("Reserved bits in TD attributes are set")
        if (!septVeDisable) dcapFail("SEPT_VE_DISABLE is not enabled")
    }

    private fun verifyRaw64(
        key: ECPublicKeyParameters,
        signature: ByteArray,
        data: ByteArray,
    ): Boolean {
        if (signature.size != 64) dcapFail("Key length is invalid: expected 64, got ${signature.size}")
        return verifyEcdsaRaw(key, Hash.SHA256, BigInteger(1, signature.copyOfRange(0, 32)), BigInteger(1, signature.copyOfRange(32, 64)), data)
    }

    private fun le32(bytes: ByteArray): Long {
        if (bytes.size < 4) dcapFail("QE MISCSELECT is not 4 bytes")
        return (0 until 4).fold(0L) { acc, i -> acc or ((bytes[i].toLong() and 0xff) shl (8 * i)) }
    }

    private fun hex32(value: Long): String = value.toString(16).padStart(8, '0').uppercase()

    private fun hex8(value: Int): String = value.toString(16).padStart(2, '0').uppercase()
}

/** A DCAP check failed; the message is dcap-qvl's. */
private fun dcapFail(message: String): Nothing = throw TeeTrustException(message)

private inline fun <T> guard(
    message: String,
    block: () -> T,
): T =
    try {
        block()
    } catch (e: TeeTrustException) {
        throw e
    } catch (e: Exception) {
        throw TeeTrustException("$message: ${e.message}", e)
    }

// ---- TCB info and QE identity ---------------------------------------------------------

private class TcbStatus(
    status: String?,
    val advisoryIds: List<String>,
) {
    val status: String = status ?: "Unknown"

    val isValid: Boolean get() = status in VALID

    fun merge(other: TcbStatus): TcbStatus {
        val worse = if (severity(other.status) > severity(status)) other.status else status
        return TcbStatus(worse, (advisoryIds + other.advisoryIds).distinct())
    }

    private companion object {
        val VALID = TCB_STATUSES.toSet()

        fun severity(status: String): Int =
            when (status) {
                "Revoked" -> 6
                in TCB_STATUSES -> TCB_STATUSES.indexOf(status)
                else -> 100
            }
    }
}

private class TcbLevel(
    val sgxComponents: List<Int>,
    val tdxComponents: List<Int>,
    val pceSvn: Int,
    val tcbStatus: String?,
    val advisoryIds: List<String>,
)

private class TcbInfo(
    val id: String?,
    val version: Int,
    val issueDate: String,
    val nextUpdate: String,
    val fmspc: String,
    val tcbLevels: List<TcbLevel>,
) {
    companion object {
        fun parse(
            json: Json,
            text: String,
        ): TcbInfo {
            val o = json.parseToJsonElement(text) as JsonObject
            val levels =
                (o["tcbLevels"] as JsonArray).map { l ->
                    val level = l as JsonObject
                    val tcb = level["tcb"] as JsonObject
                    TcbLevel(
                        sgxComponents = svns(tcb["sgxtcbcomponents"] as JsonArray),
                        tdxComponents = (tcb["tdxtcbcomponents"] as? JsonArray)?.let(::svns).orEmpty(),
                        pceSvn = (tcb["pcesvn"] as JsonPrimitive).intOrNull ?: error("pcesvn"),
                        tcbStatus = level.string("tcbStatus"),
                        advisoryIds = advisories(level),
                    )
                }
            return TcbInfo(
                id = o.string("id"),
                version = (o["version"] as JsonPrimitive).intOrNull ?: error("version"),
                issueDate = o.string("issueDate") ?: error("issueDate"),
                nextUpdate = o.string("nextUpdate") ?: error("nextUpdate"),
                fmspc = o.string("fmspc") ?: error("fmspc"),
                tcbLevels = levels,
            )
        }
    }
}

private class QeTcbLevel(
    val isvSvn: Int,
    val tcbStatus: String?,
    val advisoryIds: List<String>,
)

@Suppress("LongParameterList")
private class QeIdentity(
    val id: String?,
    val version: Int,
    val issueDate: String,
    val nextUpdate: String,
    val miscselect: String,
    val miscselectMask: String,
    val attributes: String,
    val attributesMask: String,
    val mrsigner: String,
    val isvprodid: Int,
    val tcbLevels: List<QeTcbLevel>,
) {
    companion object {
        fun parse(
            json: Json,
            text: String,
        ): QeIdentity {
            val o = json.parseToJsonElement(text) as JsonObject

            fun s(key: String) = o.string(key) ?: error(key)
            return QeIdentity(
                id = o.string("id"),
                version = (o["version"] as JsonPrimitive).intOrNull ?: error("version"),
                issueDate = s("issueDate"),
                nextUpdate = s("nextUpdate"),
                miscselect = s("miscselect"),
                miscselectMask = s("miscselectMask"),
                attributes = s("attributes"),
                attributesMask = s("attributesMask"),
                mrsigner = s("mrsigner"),
                isvprodid = (o["isvprodid"] as JsonPrimitive).intOrNull ?: error("isvprodid"),
                tcbLevels =
                    (o["tcbLevels"] as JsonArray).map { l ->
                        val level = l as JsonObject
                        QeTcbLevel(
                            isvSvn = ((level["tcb"] as JsonObject)["isvsvn"] as JsonPrimitive).intOrNull ?: error("isvsvn"),
                            tcbStatus = level.string("tcbStatus"),
                            advisoryIds = advisories(level),
                        )
                    },
            )
        }
    }
}

private fun svns(components: JsonArray): List<Int> = components.map { ((it as JsonObject)["svn"] as JsonPrimitive).intOrNull ?: error("svn") }

private fun advisories(level: JsonObject): List<String> = (level["advisoryIDs"] as? JsonArray)?.map { (it as JsonPrimitive).content }.orEmpty()

// ---- Quotes -----------------------------------------------------------------------------

internal class QuoteHeader(
    val version: Int,
    val attestationKeyType: Int,
    val teeType: Long,
)

internal class CertificationData(
    val certType: Int,
    val body: ByteArray,
)

/** dcap-qvl `AuthDataV3` (a v4 quote's auth data is read into this shape). */
internal class AuthData(
    val ecdsaSignature: ByteArray,
    val ecdsaAttestationKey: ByteArray,
    val qeReport: ByteArray,
    val qeReportSignature: ByteArray,
    val qeAuthData: ByteArray,
    val certificationData: CertificationData,
)

internal class ParsedQuote(
    val header: QuoteHeader,
    val report: DcapReport,
    val authData: AuthData,
) {
    val signedLength: Int
        get() {
            val body =
                when (report) {
                    is DcapReport.Sgx -> ENCLAVE_REPORT_BYTE_LEN
                    is DcapReport.Td10 -> TD_REPORT10_BYTE_LEN
                    is DcapReport.Td15 -> TD_REPORT15_BYTE_LEN
                }
            return HEADER_BYTE_LEN + body + if (header.version == 5) BODY_BYTE_SIZE else 0
        }

    companion object {
        const val HEADER_BYTE_LEN = 48
        const val ENCLAVE_REPORT_BYTE_LEN = 384
        const val TD_REPORT10_BYTE_LEN = 584
        const val TD_REPORT15_BYTE_LEN = 648
        const val BODY_BYTE_SIZE = 6

        fun parse(bytes: ByteArray): ParsedQuote {
            val r = QuoteReader(bytes)
            val header = QuoteHeader(r.u16(), r.u16(), r.u32())
            r.u16() // qe svn
            r.u16() // pce svn
            r.bytes(16) // qe vendor id
            r.bytes(20) // user data
            val report: DcapReport =
                when (header.version) {
                    3 -> {
                        if (header.teeType != Dcap.TEE_TYPE_SGX) dcapFail("Invalid tee type for version 3")
                        DcapReport.Sgx(r.enclaveReport())
                    }
                    4 ->
                        when (header.teeType) {
                            Dcap.TEE_TYPE_SGX -> DcapReport.Sgx(r.enclaveReport())
                            Dcap.TEE_TYPE_TDX -> DcapReport.Td10(r.td10())
                            else -> dcapFail("Invalid TEE type")
                        }
                    5 -> {
                        val bodyType = r.u16()
                        r.u32() // body size
                        when (bodyType) {
                            1 -> DcapReport.Sgx(r.enclaveReport())
                            2 -> DcapReport.Td10(r.td10())
                            3 -> DcapReport.Td15(TdReport15(r.td10(), r.bytes(16), r.bytes(48)))
                            else -> dcapFail("Unsupported body type")
                        }
                    }
                    else -> dcapFail("Unsupported quote version")
                }
            val authBytes = r.bytes(r.u32().toIntChecked())
            val auth = QuoteReader(authBytes)
            val authVersion = if (header.version == 5) 4 else header.version
            val authData =
                if (authVersion == 3) {
                    AuthData(auth.bytes(64), auth.bytes(64), auth.bytes(384), auth.bytes(64), auth.bytes(auth.u16()), auth.certificationData())
                } else {
                    val signature = auth.bytes(64)
                    val key = auth.bytes(64)
                    val outer = auth.certificationData()
                    val inner = QuoteReader(outer.body)
                    AuthData(signature, key, inner.bytes(384), inner.bytes(64), inner.bytes(inner.u16()), inner.certificationData())
                }
            return ParsedQuote(header, report, authData)
        }

        private fun Long.toIntChecked(): Int = if (this > Int.MAX_VALUE) dcapFail("Not enough data to fill buffer") else toInt()
    }
}

/** dcap-qvl's little-endian `BinaryReader`. */
internal class QuoteReader(
    private val buffer: ByteArray,
) {
    private var offset = 0

    fun bytes(length: Int): ByteArray {
        if (length < 0 || offset + length > buffer.size) dcapFail("Not enough data to fill buffer")
        return buffer.copyOfRange(offset, offset + length).also { offset += length }
    }

    fun u16(): Int = bytes(2).let { (it[0].toInt() and 0xff) or ((it[1].toInt() and 0xff) shl 8) }

    fun u32(): Long = bytes(4).let { b -> (0 until 4).fold(0L) { acc, i -> acc or ((b[i].toLong() and 0xff) shl (8 * i)) } }

    fun certificationData(): CertificationData {
        val type = u16()
        return CertificationData(type, bytes(u32().let { if (it > Int.MAX_VALUE) dcapFail("Not enough data to fill buffer") else it.toInt() }))
    }

    fun enclaveReport(): SgxEnclaveReport {
        val cpuSvn = bytes(16)
        val miscSelect = u32()
        bytes(28)
        val attributes = bytes(16)
        val mrEnclave = bytes(32)
        bytes(32)
        val mrSigner = bytes(32)
        bytes(96)
        val isvProdId = u16()
        val isvSvn = u16()
        bytes(60)
        return SgxEnclaveReport(cpuSvn, miscSelect, attributes, mrEnclave, mrSigner, isvProdId, isvSvn, bytes(64))
    }

    fun td10(): TdReport10 =
        TdReport10(
            teeTcbSvn = bytes(16),
            mrSeam = bytes(48),
            mrSignerSeam = bytes(48),
            seamAttributes = bytes(8),
            tdAttributes = bytes(8),
            xfam = bytes(8),
            mrTd = bytes(48),
            mrConfigId = bytes(48),
            mrOwner = bytes(48),
            mrOwnerConfig = bytes(48),
            rtMr0 = bytes(48),
            rtMr1 = bytes(48),
            rtMr2 = bytes(48),
            rtMr3 = bytes(48),
            reportData = bytes(64),
        )
}

// ---- Certificates and CRLs (dcap-qvl `utils.js`) -------------------------------------------

internal class StrictCertificate(
    val der: ByteArray,
    val asn: AsnCertificate,
) {
    val subject: ByteArray get() = asn.subject.getEncoded(ASN1Encoding.DER)
    val issuer: ByteArray get() = asn.issuer.getEncoded(ASN1Encoding.DER)
    val extensions: Extensions? get() = asn.tbsCertificate.extensions
}

internal class StrictCrl(
    val der: ByteArray,
    val asn: CertificateList,
) {
    val issuerName: ByteArray get() = asn.issuer.getEncoded(ASN1Encoding.DER)
}

@Suppress("TooManyFunctions")
internal object Pki {
    /** ecdsa-with-SHA256 / SHA384 (dcap-qvl also names SHA-224 and SHA-512, which Intel never uses). */
    private val SIGNATURE_HASHES = mapOf("1.2.840.10045.4.3.2" to Hash.SHA256, "1.2.840.10045.4.3.3" to Hash.SHA384)
    private val PEM = Regex("-----BEGIN CERTIFICATE-----\\r?\\n?([A-Za-z0-9+/=\\r\\n]+)\\r?\\n?-----END CERTIFICATE-----")

    fun extractCerts(chain: String): List<ByteArray> = PEM.findAll(chain).map { Base64.decode(it.groupValues[1].replace("\r", "").replace("\n", "")) }.toList()

    fun certificate(der: ByteArray): StrictCertificate {
        if (der.isEmpty()) dcapFail("Invalid certificate: empty input")
        val asn =
            try {
                AsnCertificate.getInstance(ASN1Primitive.fromByteArray(der))
            } catch (e: Exception) {
                dcapFail("Failed to parse certificate: ${e.message}")
            }
        if (!asn.getEncoded(ASN1Encoding.DER).contentEquals(der)) dcapFail("Failed to parse certificate: non-canonical or trailing DER data")
        if (asn.tbsCertificate.signature != asn.signatureAlgorithm) dcapFail("Certificate signature algorithm mismatch")
        return StrictCertificate(der, asn)
    }

    fun crl(der: ByteArray): StrictCrl {
        if (der.isEmpty()) dcapFail("Invalid CRL: empty input")
        val asn =
            try {
                CertificateList.getInstance(ASN1Primitive.fromByteArray(der))
            } catch (e: Exception) {
                dcapFail("Failed to parse CRL: ${e.message}")
            }
        if (!asn.getEncoded(ASN1Encoding.DER).contentEquals(der)) dcapFail("Failed to parse CRL: non-canonical or trailing DER data")
        val tbs = asn.tbsCertList
        if (tbs.nextUpdate == null) dcapFail("CRL is missing nextUpdate")
        val version = tbsVersion(tbs.toASN1Primitive() as ASN1Sequence)
        if (version != null && version != BigInteger.ONE) dcapFail("Unsupported CRL version")
        val entries = tbs.revokedCertificates.orEmpty()
        val hasExtensions = (tbs.extensions?.extensionOIDs?.isNotEmpty() == true) || entries.any { it.extensions?.extensionOIDs?.isNotEmpty() == true }
        if (hasExtensions && version != BigInteger.ONE) dcapFail("CRL extensions require a version 2 CRL")
        if (tbs.signature != asn.signatureAlgorithm) dcapFail("CRL signature algorithm mismatch")
        tbs.extensions?.let { ext ->
            ext.criticalExtensionOIDs.firstOrNull()?.let { dcapFail("Unsupported critical CRL extension: ${it.id}") }
        }
        for (entry in entries) {
            entry.extensions
                ?.criticalExtensionOIDs
                ?.firstOrNull()
                ?.let { dcapFail("Unsupported critical CRL entry extension: ${it.id}") }
        }
        return StrictCrl(der, asn)
    }

    /** The CRL's explicit version INTEGER, or null when it is left out (v1). */
    private fun tbsVersion(tbs: ASN1Sequence): BigInteger? = (tbs.getObjectAt(0) as? ASN1Integer)?.value

    private fun requireTime(
        timeSecs: Long,
    ): Long {
        if (timeSecs < 0) dcapFail("Invalid verification time")
        return timeSecs * 1000
    }

    private fun basicConstraints(cert: StrictCertificate): BasicConstraints? {
        val ext = cert.extensions?.getExtension(Extension.basicConstraints) ?: return null
        return try {
            BasicConstraints.getInstance(ext.parsedValue)
        } catch (e: Exception) {
            dcapFail("Certificate has invalid basic constraints: ${e.message}")
        }
    }

    private fun isCa(cert: StrictCertificate): Boolean = basicConstraints(cert)?.isCA == true

    private fun isSelfIssued(cert: StrictCertificate): Boolean = cert.subject.contentEquals(cert.issuer)

    private fun requireKeyUsage(
        cert: StrictCertificate,
        usage: Int,
        description: String,
    ) {
        // RFC 5280 only constrains use when keyUsage is present.
        val ext = cert.extensions?.getExtension(Extension.keyUsage) ?: return
        val keyUsage =
            try {
                KeyUsage.getInstance(ext.parsedValue)
            } catch (e: Exception) {
                dcapFail("Certificate has invalid key usage: ${e.message}")
            }
        if (!keyUsage.hasUsages(usage)) dcapFail("Certificate is not allowed to $description")
    }

    private fun publicKey(cert: StrictCertificate): ECPublicKeyParameters =
        try {
            PublicKeyFactory.createKey(cert.asn.subjectPublicKeyInfo) as ECPublicKeyParameters
        } catch (e: Exception) {
            dcapFail("The certificate's key is not an EC key: ${e.message}")
        }

    fun p256Key(uncompressed: ByteArray): ECPublicKeyParameters {
        // An SPKI around the raw point, so the curve is read the same way as a certificate's.
        val prefix = Hex.decodeUnsized("3059301306072a8648ce3d020106082a8648ce3d030107034200", "spki prefix")
        return try {
            PublicKeyFactory.createKey(SubjectPublicKeyInfo.getInstance(prefix + uncompressed)) as ECPublicKeyParameters
        } catch (e: Exception) {
            dcapFail("ISV enclave report signature is invalid: ${e.message}")
        }
    }

    /** Verify a DER ECDSA signature made with [algorithm] by [key] over [data]. */
    @Suppress("ReturnCount")
    private fun verifyDer(
        key: ECPublicKeyParameters,
        algorithm: String,
        signature: ByteArray,
        data: ByteArray,
    ): Boolean {
        val hash = SIGNATURE_HASHES[algorithm] ?: return false
        val seq =
            try {
                ASN1Sequence.getInstance(signature)
            } catch (_: Exception) {
                return false
            }
        if (seq.size() != 2) return false
        val r = (seq.getObjectAt(0) as? ASN1Integer)?.value?.takeIf { it.signum() > 0 } ?: return false
        val s = (seq.getObjectAt(1) as? ASN1Integer)?.value?.takeIf { it.signum() > 0 } ?: return false
        return verifyEcdsaRaw(key, hash, r, s, data)
    }

    /** Verify a raw 64-byte `r ‖ s` ECDSA P-256 / SHA-256 signature by the key of the certificate [certDer]. */
    fun verifyRawWithCertificate(
        certDer: ByteArray,
        data: ByteArray,
        signature: ByteArray,
    ): Boolean {
        if (signature.size != 64) dcapFail("Key length is invalid: expected 64, got ${signature.size}")
        val key = publicKey(certificate(certDer))
        return verifyEcdsaRaw(key, Hash.SHA256, BigInteger(1, signature.copyOfRange(0, 32)), BigInteger(1, signature.copyOfRange(32, 64)), data)
    }

    fun verifyWithCertificate(
        certDer: ByteArray,
        data: ByteArray,
        signature: ByteArray,
    ): Boolean = verifyRawWithCertificate(certDer, data, signature)

    private fun validateCertificateTime(
        cert: StrictCertificate,
        timeSecs: Long,
    ) {
        val now = requireTime(timeSecs)
        val notBefore = cert.asn.startDate.date.time
        val notAfter = cert.asn.endDate.date.time
        if (now < notBefore || now > notAfter) dcapFail("Certificate is expired or not yet valid")
    }

    private fun validateCrlTime(
        crl: StrictCrl,
        timeSecs: Long,
    ) {
        val now = requireTime(timeSecs)
        val thisUpdate = crl.asn.thisUpdate.date.time
        val nextUpdate = crl.asn.nextUpdate.date.time
        if (nextUpdate < thisUpdate) dcapFail("CRL has an invalid validity window")
        if (now < thisUpdate) dcapFail("CRL is not yet valid")
        if (now > nextUpdate) dcapFail("CRL is expired")
    }

    private fun verifyCrl(
        crlDer: ByteArray,
        issuerDer: ByteArray,
        timeSecs: Long,
    ): StrictCrl {
        val crl = crl(crlDer)
        val issuer = certificate(issuerDer)
        if (!isCa(issuer)) dcapFail("CRL issuer is not a CA certificate")
        requireKeyUsage(issuer, KeyUsage.cRLSign, "sign CRLs")
        if (!crl.issuerName.contentEquals(issuer.subject)) dcapFail("CRL issuer does not match the expected issuer certificate")
        validateCrlTime(crl, timeSecs)
        val algorithm = crl.asn.signatureAlgorithm.algorithm.id
        if (algorithm !in SIGNATURE_HASHES) dcapFail("Unsupported CRL signature algorithm: $algorithm")
        if (!verifyDer(publicKey(issuer), algorithm, crl.asn.signature.octets, crl.asn.tbsCertList.getEncoded(ASN1Encoding.DER))) {
            dcapFail("CRL signature is invalid")
        }
        return crl
    }

    private fun checkRevocation(
        certDer: ByteArray,
        issuerDer: ByteArray,
        crls: List<ByteArray>,
        timeSecs: Long,
    ) {
        val cert = certificate(certDer)
        val matching = crls.map(::crl).filter { it.issuerName.contentEquals(cert.issuer) }
        if (matching.isEmpty()) dcapFail("Certificate revocation status is unknown")
        val serial = cert.asn.serialNumber.value
        for (candidate in matching) {
            val verified = verifyCrl(candidate.der, issuerDer, timeSecs)
            if (verified.asn.revokedCertificates
                    .orEmpty()
                    .any { it.userCertificate.value == serial }
            ) {
                dcapFail("Certificate is revoked")
            }
        }
    }

    @Suppress("CyclomaticComplexMethod", "LongParameterList", "NestedBlockDepth")
    private fun verifyCertificatePath(
        leafDer: ByteArray,
        intermediates: List<ByteArray>,
        timeSecs: Long,
        crls: List<ByteArray>,
        trustAnchorDer: ByteArray,
        leafMustBeCa: Boolean,
    ) {
        val anchor = certificate(trustAnchorDer)
        val path = (listOf(leafDer) + intermediates).map(::certificate)
        if (leafMustBeCa) {
            if (!isCa(path[0])) dcapFail("CRL issuer certificate is not a CA")
        } else if (isCa(path[0])) {
            dcapFail("CaUsedAsEndEntity")
        }
        for ((i, current) in path.withIndex()) {
            if (current.der.contentEquals(anchor.der)) {
                if (i != path.size - 1) dcapFail("Certificate chain continues after the trust anchor")
                continue
            }
            val issuer = path.getOrNull(i + 1) ?: anchor
            if (i > 0) {
                if (!isCa(current)) dcapFail("Non-CA certificate used as an intermediate")
                requireKeyUsage(current, KeyUsage.keyCertSign, "sign certificates")
                // RFC 5280 4.2.1.9: pathLenConstraint bounds the non-self-issued intermediates
                // that may follow this certificate in the path (the end entity is not counted).
                basicConstraints(current)?.pathLenConstraint?.let { limit ->
                    val following = path.subList(1, i).count { !isSelfIssued(it) }
                    if (following.toBigInteger() > limit) dcapFail("Certificate path length constraint violated")
                }
            }
            if (!current.issuer.contentEquals(issuer.subject)) dcapFail("Failed to verify certificate chain - issuer name mismatch")
            val algorithm = current.asn.signatureAlgorithm.algorithm.id
            if (!verifyDer(publicKey(issuer), algorithm, current.asn.signature.octets, current.asn.tbsCertificate.getEncoded(ASN1Encoding.DER))) {
                dcapFail("Failed to verify certificate chain - signature invalid")
            }
            validateCertificateTime(current, timeSecs)
            checkRevocation(current.der, issuer.der, crls, timeSecs)
        }
    }

    /** A certificate path whose every non-anchor certificate has authenticated, current revocation status. */
    fun verifyCertificateChain(
        leafDer: ByteArray,
        intermediates: List<ByteArray>,
        timeSecs: Long,
        crls: List<ByteArray>,
        trustAnchorDer: ByteArray,
    ) {
        if (crls.isEmpty()) dcapFail("CRLs are required")
        crls.forEach(::crl)
        verifyCertificatePath(leafDer, intermediates, timeSecs, crls, trustAnchorDer, leafMustBeCa = false)
    }

    /** Authenticate the two collateral CRLs before any revocation answer they give is used. */
    fun validateCollateralCrls(
        rootCaDer: ByteArray,
        pckCrlIssuerChain: String,
        rootCaCrl: ByteArray,
        pckCrl: ByteArray,
        timeSecs: Long,
    ): List<ByteArray> {
        crl(rootCaCrl)
        crl(pckCrl)
        // The root CRL must be authenticated by the trust anchor directly, and applies to the
        // root itself, as webpki's strict root revocation check does in the Rust verifier.
        verifyCrl(rootCaCrl, rootCaDer, timeSecs)
        checkRevocation(rootCaDer, rootCaDer, listOf(rootCaCrl), timeSecs)
        val issuers = extractCerts(pckCrlIssuerChain)
        if (issuers.isEmpty()) dcapFail("PCK CRL issuer chain is empty or malformed")
        verifyCertificatePath(issuers[0], issuers.drop(1), timeSecs, listOf(rootCaCrl), rootCaDer, leafMustBeCa = true)
        verifyCrl(pckCrl, issuers[0], timeSecs)
        return listOf(rootCaCrl, pckCrl)
    }

    // ---- Intel's PCK extension -----------------------------------------------------------

    fun intelExtension(certDer: ByteArray): ByteArray {
        val ext = certificate(certDer).extensions ?: dcapFail("Intel extension not found")
        val matches = ext.extensionOIDs.filter { it.id == SGX_EXTENSION_OID }
        if (matches.isEmpty()) dcapFail("Intel extension not found")
        if (matches.size > 1) dcapFail("Intel extension ambiguity")
        return ext.getExtension(matches[0]).extnValue.octets
    }

    /** The value at [path] in Intel's `SEQUENCE OF SEQUENCE { OID, ANY }`: an OCTET STRING's contents, else the whole element. */
    private fun findExtension(
        path: List<String>,
        raw: ByteArray,
    ): ByteArray {
        var current = raw
        for (oid in path) {
            val items =
                try {
                    parseDer(current).children
                } catch (e: DerException) {
                    dcapFail("OID $oid is missing: ${e.message}")
                }
            val found =
                items.firstOrNull { item ->
                    runCatching { oidOf(item.children.firstOrNull()) == oid }.getOrDefault(false)
                } ?: dcapFail("OID $oid is missing")
            current = found.children.getOrNull(1)?.whole ?: dcapFail("OID $oid is missing")
        }
        val element = runCatching { parseDer(current) }.getOrNull()
        return if (element?.tag == DER_OCTET_STRING) element.contents else current
    }

    fun fmspc(extension: ByteArray): ByteArray = findExtension(listOf(FMSPC_OID), extension).also { if (it.size != 6) dcapFail("Fmspc length mismatch") }

    fun cpuSvn(extension: ByteArray): ByteArray =
        findExtension(listOf(TCB_OID, CPUSVN_OID), extension).also { if (it.size != 16) dcapFail("CpuSvn length mismatch") }

    fun pceSvn(extension: ByteArray): Int {
        var data = findExtension(listOf(TCB_OID, PCESVN_OID), extension)
        if (data.size >= 2 && data[0].toInt() == 0x02) {
            val length = data[1].toInt() and 0xff
            data = data.copyOfRange(2, minOf(data.size, 2 + length))
        }
        return when (data.size) {
            1 -> data[0].toInt() and 0xff
            2 -> ((data[0].toInt() and 0xff) shl 8) or (data[1].toInt() and 0xff)
            else -> dcapFail("PceSvn length mismatch")
        }
    }

    private const val SGX_EXTENSION_OID = "1.2.840.113741.1.13.1"
    private const val TCB_OID = "1.2.840.113741.1.13.1.2"
    private const val FMSPC_OID = "1.2.840.113741.1.13.1.4"
    private const val PCESVN_OID = "1.2.840.113741.1.13.1.2.17"
    private const val CPUSVN_OID = "1.2.840.113741.1.13.1.2.18"
}
