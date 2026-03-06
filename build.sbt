import com.typesafe.sbt.packager.docker.DockerChmodType

lazy val root = (project in file("."))
  .enablePlugins(JavaAppPackaging, DockerPlugin)
  .settings(
    name         := "flashcards",
    version      := "0.1.0",
    scalaVersion := "3.3.4",
    organization := "com.flashcards",

    libraryDependencies ++= Seq(
      // HTTP server – minimalistyczny, zero-dep
      "com.lihaoyi"    %% "cask"       % "0.11.3",

      // Type-safe SQL DSL dla Scala 3
      "com.augustnagro" %% "magnum"    % "1.3.1",

      // Connection pool
      "com.zaxxer"      %  "HikariCP"  % "5.1.0",

      // PostgreSQL driver
      "org.postgresql"  %  "postgresql" % "42.7.4",

      // Flyway migracje
      "org.flywaydb"    %  "flyway-core"               % "10.20.1",
      "org.flywaydb"    %  "flyway-database-postgresql" % "10.20.1",

      // Logowanie
      "ch.qos.logback"  %  "logback-classic" % "1.5.12",
    ),

    // sbt-assembly – fat jar, najprostszy deployment
    assembly / mainClass          := Some("com.flashcards.Main"),
    assembly / assemblyJarName    := "flashcards.jar",
    assembly / assemblyMergeStrategy := {
      case PathList("META-INF", "services", xs @ _*) => MergeStrategy.concat
      case PathList("META-INF", xs @ _*)             => MergeStrategy.discard
      case PathList("reference.conf")                => MergeStrategy.concat
      case _                                         => MergeStrategy.first
    },

    // Docker
    Docker / packageName      := "flashcards",
    Docker / version          := "latest",
    dockerBaseImage           := "eclipse-temurin:21-jre-alpine",
    dockerChmodType           := DockerChmodType.UserGroupWriteExecute,
    dockerExposedPorts        := Seq(8080),
    dockerUpdateLatest        := true,

    scalacOptions ++= Seq("-deprecation", "-feature"),

    // Inject git SHA into the jar as a resource file
    Compile / resourceGenerators += Def.task {
      val sha  = sys.env.getOrElse("GIT_SHA", "unknown")
      val file = (Compile / resourceManaged).value / "version.properties"
      IO.write(file, s"git.sha=$sha\n")
      Seq(file)
    }.taskValue,
  )
