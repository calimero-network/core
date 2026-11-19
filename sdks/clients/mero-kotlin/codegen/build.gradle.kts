// Generates mero-core's API client from core's OpenAPI document: docs/public/openapi.json of the
// core tree this SDK lives in (sdks/clients/mero-kotlin), read in place, never fetched.
//
//   ./gradlew -p codegen generateMeroClient     # rewrite mero-core/src/generated/kotlin
//   python3 scripts/sdks.py generate mero-kotlin   # the same, from core's root
plugins {
    // Pinned exactly: the generated sources are checked in, so a generator bump is a reviewed
    // diff of its own, never a side effect of resolving a range.
    id("org.openapi.generator") version "7.26.0"
}

val spec = rootDir.resolve("../../../../docs/public/openapi.json")
val generated = layout.buildDirectory.dir("openapi")
val target = rootDir.resolve("../mero-core/src/generated/kotlin")

// What the generator reads: core's document with its `oneOf` request body named as a
// component, so schemaMappings can map it (see prepare-spec.py).
val prepared = layout.buildDirectory.file("openapi-prepared.json")
val prepareSpec =
    tasks.register<Exec>("prepareSpec") {
        inputs.files(spec, rootDir.resolve("prepare-spec.py"))
        outputs.file(prepared)
        commandLine("python3", rootDir.resolve("prepare-spec.py").path, spec.path, prepared.get().asFile.path)
    }

openApiGenerate {
    generatorName.set("kotlin")
    library.set("jvm-okhttp4")
    inputSpec.set(prepared.get().asFile.path)
    outputDir.set(generated)
    packageName.set("com.calimero.mero.api")
    apiPackage.set("com.calimero.mero.api.apis")
    modelPackage.set("com.calimero.mero.api.models")
    // Tags become the API classes (AppsApi, AccountsApi, OrganizationsApi, ...). Inline request
    // and answer schemas are named after their operation (InviteCreateRequest, ...).
    generateApiDocumentation.set(false)
    generateModelDocumentation.set(false)
    generateApiTests.set(false)
    generateModelTests.set(false)
    // The document passes the generator's validator (OpenAPI 3.1, beta support there).
    skipValidateSpec.set(false)
    templateDir.set(rootDir.resolve("templates").path)
    ignoreFileOverride.set(rootDir.resolve("openapi-generator-ignore").path)
    configOptions.putAll(
        mapOf(
            "serializationLibrary" to "kotlinx_serialization",
            "useCoroutines" to "true",
            "sourceFolder" to "src/main/kotlin",
            "enumPropertyNaming" to "UPPERCASE",
            "omitGradleWrapper" to "true",
            "omitGradlePluginVersions" to "true",
            "useSettingsGradle" to "false",
            "dateLibrary" to "string",
            "idea" to "false",
        ),
    )
    // Free-form JSON (`{}`: app arguments, error details, metadata) is a JsonElement, which
    // kotlinx.serialization reads and writes; `kotlin.Any` it cannot.
    // An answer that is `{ type: object }` and nothing more is a JsonObject (`kotlin.Any`, upstream's
    // default, kotlinx.serialization cannot read).
    // An `integer` with no format (every one in the spec: log indexes, sizes, times) is a Long: the
    // node's are u64, and a blob's size or a log index outgrows an Int.
    typeMappings.putAll(
        mapOf(
            "AnyType" to "JsonElement",
            "object" to "JsonObject",
            "integer" to "kotlin.Long",
            // `format: binary` bodies and answers (blobs, files, packages, bundles) are bytes, not a
            // java.io.File: an app has them in memory, and a temp file is no answer on Android.
            "file" to "kotlin.ByteArray",
            "binary" to "kotlin.ByteArray",
        ),
    )
    importMappings.putAll(
        mapOf(
            "JsonElement" to "kotlinx.serialization.json.JsonElement",
            "JsonObject" to "kotlinx.serialization.json.JsonObject",
        ),
    )
    // The spec's unions are mero-core's own sealed types, each with the serializer that writes
    // exactly the union's forms; the generator would flatten an `anyOf` into one class whose every
    // branch's fields are required, and renders a `oneOf` as a sealed class no branch extends.
    schemaMappings.putAll(
        mapOf(
            "Access" to "com.calimero.mero.AppAccess",
            "Deployer" to "com.calimero.mero.Deployer",
            "OutboundSelection" to "com.calimero.mero.OutboundSelection",
            // The consent request body is a `oneOf` with a `kind` discriminator, which 7.26's
            // kotlinx output renders as a sealed class that no branch extends.
            "ConsentPageCreateRequest" to "com.calimero.mero.ConsentRequest",
        ),
    )
    inlineSchemaOptions.putAll(mapOf("SKIP_SCHEMA_REUSE" to "true"))
    additionalProperties.putAll(mapOf("hideGenerationTimestamp" to "true"))
}

tasks.named("openApiGenerate") {
    dependsOn(prepareSpec)
    // Always from a clean directory: a schema core removed must not survive as a stale file.
    doFirst { delete(generated) }
}

val syncMeroClient =
    tasks.register<Sync>("syncMeroClient") {
        dependsOn("openApiGenerate")
        from(generated.map { it.dir("src/main/kotlin") })
        into(target)
    }

// The facade (`client.orgs.inviteCreate(org = ..., role = ...)`) over the generated client: one
// suspend method per operation, the body's fields flattened. Written by codegen/facade.py, which
// reads the spec and the generated sources it calls, so it cannot drift from them.
tasks.register<Exec>("generateMeroClient") {
    group = "codegen"
    description = "Regenerates mero-core/src/generated/kotlin (client and facade) from core's docs/public/openapi.json"
    dependsOn(syncMeroClient)
    commandLine("python3", rootDir.resolve("facade.py").path, spec.path, target.path)
}
