lazy val root = (project in file("."))
  .enablePlugins(JavaAppPackaging, GraalVMNativeImagePlugin)
  .settings(
    name         := "flashcards",
    version      := "0.1.0",
    scalaVersion := "3.3.4",
    organization := "com.flashcards",

    libraryDependencies ++= Seq(
      // HTTP server
      "com.lihaoyi"     %% "cask"       % "0.11.3",

      // Type-safe SQL DSL for Scala 3
      "com.augustnagro" %% "magnum"     % "1.3.1",

      // Connection pool
      "com.zaxxer"      %  "HikariCP"   % "5.1.0",

      // PostgreSQL driver
      "org.postgresql"  %  "postgresql" % "42.7.4",

      // Flyway migrations
      "org.flywaydb"    %  "flyway-core"               % "10.20.1",
      "org.flywaydb"    %  "flyway-database-postgresql" % "10.20.1",

      // Logging
      "ch.qos.logback"  %  "logback-classic" % "1.5.12",
    ),

    // GraalVM native-image
    GraalVMNativeImage / mainClass := Some("com.flashcards.Main"),
    graalVMNativeImageOptions ++= Seq(
      "--no-fallback",
      "-H:+StaticExecutableWithDynamicLibC",
      "--enable-url-protocols=http,https",
      "-H:+ReportExceptionStackTraces",
      "--initialize-at-build-time=org.slf4j",
      "--initialize-at-build-time=ch.qos.logback",
      "-march=compatibility",
    ),

    scalacOptions ++= Seq("-deprecation", "-feature"),

    // Inject git SHA into the binary via a resource file
    Compile / resourceGenerators += Def.task {
      val sha  = sys.env.getOrElse("GIT_SHA", "unknown")
      val file = (Compile / resourceManaged).value / "version.properties"
      IO.write(file, s"git.sha=$sha\n")
      Seq(file)
    }.taskValue,
  )
