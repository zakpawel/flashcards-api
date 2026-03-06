import com.typesafe.sbt.packager.docker.DockerChmodType

val zioVersion        = "2.1.13"
val zioHttpVersion    = "3.0.1"
val tapirVersion      = "1.11.9"
val skunkVersion      = "0.6.4"
val zioJsonVersion    = "0.7.3"
val flywayVersion     = "10.20.1"
val postgresVersion   = "42.7.4"
val zioConfigVersion  = "4.0.3"
val logbackVersion    = "1.5.12"

lazy val root = (project in file("."))
  .enablePlugins(JavaAppPackaging, DockerPlugin)
  .settings(
    name         := "flashcards",
    version      := "0.1.0",
    scalaVersion := "3.3.4",
    organization := "com.flashcards",

    libraryDependencies ++= Seq(
      // ZIO core
      "dev.zio" %% "zio"         % zioVersion,
      "dev.zio" %% "zio-streams" % zioVersion,

      // ZIO HTTP
      "dev.zio" %% "zio-http" % zioHttpVersion,

      // Tapir – endpoints + ZIO HTTP server + OpenAPI
      "com.softwaremill.sttp.tapir" %% "tapir-zio-http-server"  % tapirVersion,
      "com.softwaremill.sttp.tapir" %% "tapir-json-zio"         % tapirVersion,
      "com.softwaremill.sttp.tapir" %% "tapir-swagger-ui-bundle" % tapirVersion,
      "com.softwaremill.sttp.tapir" %% "tapir-openapi-docs"     % tapirVersion,

      // Skunk – type-safe Postgres DSL (pure functional, no JDBC)
      "org.tpolecat" %% "skunk-core"       % skunkVersion,
      "dev.zio"      %% "zio-interop-catz" % "23.1.0.3",

      // ZIO JSON
      "dev.zio" %% "zio-json" % zioJsonVersion,

      // Flyway migrations (JDBC only for startup migration)
      "org.flywaydb"   % "flyway-core"                % flywayVersion,
      "org.flywaydb"   % "flyway-database-postgresql"  % flywayVersion,
      "org.postgresql" % "postgresql"                  % postgresVersion,

      // ZIO Config – czyta env vars / application.conf
      "dev.zio" %% "zio-config"          % zioConfigVersion,
      "dev.zio" %% "zio-config-typesafe" % zioConfigVersion,
      "dev.zio" %% "zio-config-magnolia" % zioConfigVersion,

      // Logging
      "ch.qos.logback" % "logback-classic" % logbackVersion,
      "dev.zio"       %% "zio-logging-slf4j2" % "2.3.2",
    ),

    // Docker / Render deployment
    Docker / packageName       := "flashcards",
    Docker / version           := "latest",
    dockerBaseImage            := "eclipse-temurin:21-jre-alpine",
    dockerChmodType            := DockerChmodType.UserGroupWriteExecute,
    dockerExposedPorts         := Seq(8080),
    dockerUpdateLatest         := true,

    scalacOptions ++= Seq(
      "-deprecation",
      "-feature",
      "-Wunused:all",
    ),
  )
