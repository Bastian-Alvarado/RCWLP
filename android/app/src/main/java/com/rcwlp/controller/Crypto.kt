package com.rcwlp.controller

import java.math.BigInteger
import java.nio.ByteBuffer
import java.security.AlgorithmParameters
import java.security.KeyFactory
import java.security.KeyPair
import java.security.KeyPairGenerator
import java.security.MessageDigest
import java.security.SecureRandom
import java.security.interfaces.ECPublicKey
import java.security.spec.ECGenParameterSpec
import java.security.spec.ECParameterSpec
import java.security.spec.ECPoint
import java.security.spec.ECPrivateKeySpec
import java.security.spec.ECPublicKeySpec
import javax.crypto.KeyAgreement
import javax.crypto.Mac
import javax.crypto.spec.SecretKeySpec

/** Mirrors host/src/crypto.rs: P-256 ECDH for pairing, HMAC-SHA256 for every packet after that. */
object Crypto {
    private val rng = SecureRandom()

    fun random(n: Int): ByteArray = ByteArray(n).also { rng.nextBytes(it) }

    fun newMac(key: ByteArray): Mac = Mac.getInstance("HmacSHA256").apply { init(SecretKeySpec(key, "HmacSHA256")) }

    fun hmac(key: ByteArray, vararg parts: ByteArray): ByteArray {
        val mac = newMac(key)
        parts.forEach(mac::update)
        return mac.doFinal()
    }

    fun mac16(key: ByteArray, data: ByteArray): ByteArray = hmac(key, data).copyOf(Proto.MAC_LEN)

    fun verify16(key: ByteArray, data: ByteArray, mac: ByteArray): Boolean =
        MessageDigest.isEqual(mac16(key, data), mac)

    /**
     * What the pairing hello carries instead of the public key. The phone is bound to its key
     * before it sees the host's, so a man in the middle can't search for a host key that makes
     * both screens show the same code.
     */
    fun commitment(publicKey: ByteArray): ByteArray = MessageDigest.getInstance("SHA-256").run {
        update("rcwlp-commit".toByteArray())
        update(publicKey)
        digest()
    }

    fun pairKey(shared: ByteArray, clientId: ByteArray, hostId: ByteArray, clientPub: ByteArray, hostPub: ByteArray) =
        hmac(shared, "rcwlp-pair-v1".toByteArray(), clientId, hostId, clientPub, hostPub)

    /** Six-digit code shown on both sides so the user can confirm they derived the same key. */
    fun pairCode(pairKey: ByteArray): Int {
        val h = hmac(pairKey, "rcwlp-code".toByteArray())
        return ((ByteBuffer.wrap(h).int.toLong() and 0xffffffffL) % 1_000_000).toInt()
    }

    fun pairOkMac(pairKey: ByteArray) = mac16(pairKey, "rcwlp-pair-ok".toByteArray())

    fun sessionKey(pairKey: ByteArray, cnonce: ByteArray, hnonce: ByteArray) =
        hmac(pairKey, "rcwlp-session".toByteArray(), cnonce, hnonce)

    /** A P-256 key pair for one pairing attempt. */
    class EphemeralKey(private val pair: KeyPair = generate()) {
        private val params = (pair.public as ECPublicKey).params

        /** Uncompressed SEC1 point: 0x04 || x || y. */
        val publicBytes: ByteArray = (pair.public as ECPublicKey).w.let {
            byteArrayOf(4) + it.affineX.to32() + it.affineY.to32()
        }

        fun sharedSecret(peer: ByteArray): ByteArray {
            val peerKey = KeyFactory.getInstance("EC").generatePublic(ECPublicKeySpec(point(peer), params))
            return KeyAgreement.getInstance("ECDH").run {
                init(pair.private)
                doPhase(peerKey, true)
                generateSecret()
            }
        }

        companion object {
            private fun generate(): KeyPair =
                KeyPairGenerator.getInstance("EC").apply { initialize(ECGenParameterSpec("secp256r1")) }.generateKeyPair()

            /** Rebuilds a known key pair, for the protocol test vectors. */
            fun fromRaw(privateKey: ByteArray, publicKey: ByteArray): EphemeralKey {
                val spec = AlgorithmParameters.getInstance("EC")
                    .apply { init(ECGenParameterSpec("secp256r1")) }
                    .getParameterSpec(ECParameterSpec::class.java)
                val factory = KeyFactory.getInstance("EC")
                return EphemeralKey(
                    KeyPair(
                        factory.generatePublic(ECPublicKeySpec(point(publicKey), spec)),
                        factory.generatePrivate(ECPrivateKeySpec(BigInteger(1, privateKey), spec)),
                    ),
                )
            }

            private fun point(sec1: ByteArray): ECPoint {
                require(sec1.size == Proto.PUB_LEN && sec1[0] == 4.toByte()) { "bad public key" }
                return ECPoint(BigInteger(1, sec1.copyOfRange(1, 33)), BigInteger(1, sec1.copyOfRange(33, 65)))
            }

            private fun BigInteger.to32(): ByteArray {
                val raw = toByteArray()
                return when {
                    raw.size == 32 -> raw
                    raw.size > 32 -> raw.copyOfRange(raw.size - 32, raw.size)
                    else -> ByteArray(32 - raw.size) + raw
                }
            }
        }
    }
}
