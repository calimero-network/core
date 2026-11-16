# kotlinx.serialization: keep generated serializers for @Serializable DTOs.
-keepattributes *Annotation*, InnerClasses
-dontnote kotlinx.serialization.**

-if @kotlinx.serialization.Serializable class **
-keepclassmembers class <1> {
    static <1>$Companion Companion;
}
-if @kotlinx.serialization.Serializable class ** {
    static **$* *;
}
-keepclassmembers class <2>$<3> {
    kotlinx.serialization.KSerializer serializer(...);
}

# Bouncy Castle (Ed25519 / X25519 for the Cloud account layer). The SDK calls the lightweight
# API directly, so R8 can shrink it; it only needs quieting about the JCA/JNDI/LDAP classes BC
# references for features the SDK never touches.
-dontwarn org.bouncycastle.**
-dontwarn javax.naming.**
