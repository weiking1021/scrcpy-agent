package io.github.weiking1021.scrcpyagent.android;

import android.content.Context;
import io.github.muntashirakon.adb.AbsAdbConnectionManager;
import org.bouncycastle.asn1.x500.X500Name;
import org.bouncycastle.cert.jcajce.JcaX509v3CertificateBuilder;
import org.bouncycastle.operator.jcajce.JcaContentSignerBuilder;
import org.conscrypt.Conscrypt;
import java.io.ByteArrayInputStream;
import java.io.File;
import java.math.BigInteger;
import java.nio.file.Files;
import java.security.KeyFactory;
import java.security.KeyPairGenerator;
import java.security.PrivateKey;
import java.security.SecureRandom;
import java.security.Security;
import java.security.cert.Certificate;
import java.security.cert.CertificateFactory;
import java.security.spec.PKCS8EncodedKeySpec;
import java.util.Date;
import java.util.concurrent.TimeUnit;

/** Direct adbd connection, including Android 11+ TLS pairing; no PC adb server. */
final class AdbClient extends AbsAdbConnectionManager {
    private final PrivateKey key;
    private final Certificate certificate;

    AdbClient(Context context) throws Exception {
        if (Security.getProvider("Conscrypt") == null) Security.insertProviderAt(Conscrypt.newProvider(), 1);
        var keyFile = new File(context.getNoBackupFilesDir(), "adb-private.pk8").toPath();
        var certFile = new File(context.getNoBackupFilesDir(), "adb-certificate.der").toPath();
        if (!Files.isRegularFile(keyFile) || !Files.isRegularFile(certFile)) {
            // This is the device's ADB authentication identity, not an APK signing key.
            var generator = KeyPairGenerator.getInstance("RSA");
            generator.initialize(2048);
            var pair = generator.generateKeyPair();
            var name = new X500Name("CN=scrcpy-agent Android");
            long now = System.currentTimeMillis();
            var builder = new JcaX509v3CertificateBuilder(name, new BigInteger(128, new SecureRandom()),
                new Date(now - 86400000L), new Date(now + 10L * 365 * 86400000L), name, pair.getPublic());
            var cert = builder.build(new JcaContentSignerBuilder("SHA256withRSA").build(pair.getPrivate()));
            Files.write(keyFile, pair.getPrivate().getEncoded());
            Files.write(certFile, cert.getEncoded());
        }
        key = KeyFactory.getInstance("RSA").generatePrivate(new PKCS8EncodedKeySpec(Files.readAllBytes(keyFile)));
        certificate = CertificateFactory.getInstance("X.509").generateCertificate(new ByteArrayInputStream(Files.readAllBytes(certFile)));
        // Remote Android 11+ is the baseline, independent of this phone's API level.
        setApi(30);
        setTimeout(15, TimeUnit.SECONDS);
    }

    @Override protected PrivateKey getPrivateKey() { return key; }
    @Override protected Certificate getCertificate() { return certificate; }
    @Override protected String getDeviceName() { return "scrcpy-agent Android"; }
}
