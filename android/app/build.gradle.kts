// AGP 9 compiles Kotlin itself, so no separate Kotlin plugin is applied.
plugins {
    id("com.android.application")
}

// Wire-format test vectors shared with the PC host, at the repo root.
val protocolVectors = rootProject.file("../protocol-vectors.txt")


android {
    namespace = "com.rcwlp.controller"
    compileSdk = 36
    ndkVersion = "28.2.13676358"

    defaultConfig {
        applicationId = "com.rcwlp.controller"
        minSdk = 26
        targetSdk = 36
        versionCode = 2
        versionName = "0.2.0"

        ndk {
            abiFilters += listOf("arm64-v8a", "armeabi-v7a", "x86_64")
        }
    }

    buildFeatures {
        aidl = true // Interface to the background-mode input reader (EvdevService)
        buildConfig = true
    }

    // Raw input access for background mode, see src/main/cpp/evdev.c.
    externalNativeBuild {
        cmake {
            path = file("src/main/cpp/CMakeLists.txt")
            version = "3.22.1"
        }
    }

    buildTypes {
        release {
            isMinifyEnabled = false
        }
    }

    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }

    testOptions {
        unitTests.all {
            it.systemProperty("rcwlp.vectors", protocolVectors.absolutePath)
            it.inputs.file(protocolVectors)
        }
    }
}

dependencies {
    // Background mode: runs EvdevService with adb-level privileges, see ControllerService.
    implementation("dev.rikka.shizuku:api:13.1.5")
    implementation("dev.rikka.shizuku:provider:13.1.5")
    implementation("androidx.annotation:annotation:1.9.1")

    testImplementation("junit:junit:4.13.2")
}
