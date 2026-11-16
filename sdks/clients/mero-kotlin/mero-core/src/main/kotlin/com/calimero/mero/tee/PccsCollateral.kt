package com.calimero.mero.tee

import com.calimero.mero.crypto.Hex
import com.calimero.mero.http.NetworkException
import com.calimero.mero.http.PlainJsonHttp
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext
import kotlinx.serialization.json.Json
import kotlinx.serialization.json.JsonObject
import okhttp3.OkHttpClient
import okhttp3.Request
import org.bouncycastle.asn1.ASN1Encoding
import org.bouncycastle.asn1.x509.CRLDistPoint
import org.bouncycastle.asn1.x509.Extension
import org.bouncycastle.asn1.x509.GeneralName
import java.io.IOException
import java.net.URLDecoder

/**
 * Fetching a quote's DCAP collateral from a PCCS — port of `getCollateral` from
 * `@phala/dcap-qvl` 0.5.3 (`collateral.js`), what mero-js passes as `fetchCollateral` when the
 * other side sends none (an agent gate). The collateral is Intel-signed and verified by [Dcap]
 * before anything in it is used, so the PCCS is only a transport.
 *
 * Supports quotes that carry their PCK certificate chain (certification data type 5, every TDX
 * quote GCP produces); a quote with an encrypted PPID instead (types 2 and 3) is refused.
 */
object PccsCollateral {
    /** Phala Network's PCCS, dcap-qvl's default. */
    const val PHALA_PCCS_URL: String = "https://pccs.phala.network"

    /** Intel's own PCS. */
    const val INTEL_PCS_URL: String = "https://api.trustedservices.intel.com"

    private val json = Json { ignoreUnknownKeys = true }
    private val SUFFIX = Regex("/(sgx|tdx)/certification/v4$")

    /**
     * The collateral for [quote], from [pccsUrl]: the PCK CRL and its issuer chain, the TCB info
     * for the platform's FMSPC, the QE identity, and the root CA CRL — with the quote's own PCK
     * chain attached.
     */
    suspend fun fetch(
        quote: ByteArray,
        pccsUrl: String = PHALA_PCCS_URL,
        httpClient: OkHttpClient = PlainJsonHttp.defaultClient(),
    ): DcapCollateral {
        val parsed =
            try {
                ParsedQuote.parse(quote)
            } catch (e: TeeTrustException) {
                throw e
            } catch (e: Exception) {
                throw TeeTrustException("Failed to decode quote: ${e.message}", e)
            }
        val certData = parsed.authData.certificationData
        if (certData.certType != PCK_CERT_CHAIN) refuse("Unsupported certification data type: ${certData.certType}")
        val pckChain = String(certData.body, Charsets.UTF_8)
        val leaf = Pki.extractCerts(pckChain).firstOrNull() ?: refuse("Empty certificate chain")
        val fmspc = Hex.encode(Pki.fmspc(Pki.intelExtension(leaf))).uppercase()
        val issuer =
            Pki
                .certificate(leaf)
                .asn.issuer
                .toString()
        val ca = if (!issuer.contains("Processor") && issuer.contains("Platform")) "platform" else "processor"
        val tee = if (parsed.header.teeType == Dcap.TEE_TYPE_SGX) "sgx" else "tdx"
        val base = pccsUrl.trimEnd('/').replace(SUFFIX, "")
        val url = { kind: String, path: String -> "$base/$kind/certification/v4/$path" }

        val pckCrl = get(httpClient, url("sgx", "pckcrl?ca=$ca&encoding=der"), "PCK CRL")
        val tcb = get(httpClient, url(tee, "tcb?fmspc=$fmspc"), "TCB info")
        val qe = get(httpClient, url(tee, "qe/identity?update=standard"), "QE identity")
        val qeChain = qe.header("SGX-Enclave-Identity-Issuer-Chain")
        val rootCaCrl =
            (if (base.startsWith(INTEL_PCS_URL)) null else runCatching { Hex.decodeUnsized(String(get(httpClient, url("sgx", "rootcacrl"), "root CA CRL").body).trim(), "rootcacrl") }.getOrNull())
                ?: get(httpClient, crlUrlOf(Pki.extractCerts(qeChain).lastOrNull() ?: refuse("No certificates found in QE identity issuer chain")), "root CA CRL").body

        val tcbJson = json.parseToJsonElement(String(tcb.body, Charsets.UTF_8)) as JsonObject
        val qeJson = json.parseToJsonElement(String(qe.body, Charsets.UTF_8)) as JsonObject
        return DcapCollateral(
            pckCrlIssuerChain = pckCrl.header("SGX-PCK-CRL-Issuer-Chain"),
            rootCaCrl = rootCaCrl,
            pckCrl = pckCrl.body,
            tcbInfoIssuerChain = tcb.headerOrNull("SGX-TCB-Info-Issuer-Chain") ?: tcb.header("TCB-Info-Issuer-Chain"),
            tcbInfo = tcbJson["tcbInfo"].toString(),
            tcbInfoSignature = Hex.decodeUnsized(tcbJson.string("signature") ?: refuse("The TCB info has no signature"), "signature"),
            qeIdentityIssuerChain = qeChain,
            qeIdentity = qeJson["enclaveIdentity"].toString(),
            qeIdentitySignature = Hex.decodeUnsized(qeJson.string("signature") ?: refuse("The QE identity has no signature"), "signature"),
            pckCertificateChain = pckChain,
        )
    }

    private const val PCK_CERT_CHAIN = 5

    private class Fetched(
        val body: ByteArray,
        val headers: okhttp3.Headers,
    ) {
        fun headerOrNull(name: String): String? = headers[name]?.let { URLDecoder.decode(it, "UTF-8") }

        fun header(name: String): String = headerOrNull(name) ?: refuse("Missing $name")
    }

    private suspend fun get(
        client: OkHttpClient,
        url: String,
        what: String,
    ): Fetched =
        withContext(Dispatchers.IO) {
            try {
                client.newCall(Request.Builder().url(url).build()).execute().use { response ->
                    if (!response.isSuccessful) refuse("Failed to fetch $what: ${response.code}")
                    Fetched(response.body?.bytes() ?: ByteArray(0), response.headers)
                }
            } catch (e: IOException) {
                throw NetworkException("Failed to fetch $what from $url: ${e.message}", e)
            }
        }

    /** The CRL distribution point URL in the certificate [der]. */
    private fun crlUrlOf(der: ByteArray): String {
        val ext = Pki.certificate(der).extensions?.getExtension(Extension.cRLDistributionPoints) ?: refuse("Could not find CRL distribution point in root certificate")
        val points = CRLDistPoint.getInstance(ext.parsedValue).distributionPoints
        for (point in points) {
            val names =
                point.distributionPoint
                    ?.name
                    ?.toASN1Primitive()
                    ?.getEncoded(ASN1Encoding.DER) ?: continue
            val general =
                org.bouncycastle.asn1.x509.GeneralNames
                    .getInstance(
                        org.bouncycastle.asn1.ASN1Primitive
                            .fromByteArray(names),
                    )
            general.names.firstOrNull { it.tagNo == GeneralName.uniformResourceIdentifier }?.let { return it.name.toString() }
        }
        refuse("Could not find CRL distribution point in root certificate")
    }
}
