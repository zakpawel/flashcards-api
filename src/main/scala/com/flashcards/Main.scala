package com.flashcards

import com.flashcards.api.{Endpoints, Routes}
import com.flashcards.config.AppConfig
import com.flashcards.repository.FlashcardRepository
import org.flywaydb.core.Flyway
import sttp.tapir.server.ziohttp.ZioHttpInterpreter
import sttp.tapir.swagger.bundle.SwaggerInterpreter
import zio.*
import zio.http.*
import zio.logging.backend.SLF4J

object Main extends ZIOAppDefault:

  override val bootstrap: ZLayer[ZIOAppArgs, Any, Any] =
    Runtime.removeDefaultLoggers >>> SLF4J.slf4j

  // Run Flyway migrations on startup (uses plain JDBC)
  private val runMigrations: ZIO[AppConfig, Throwable, Unit] =
    ZIO.serviceWithZIO[AppConfig] { cfg =>
      ZIO.attempt {
        Flyway
          .configure()
          .dataSource(
            s"jdbc:postgresql://${cfg.db.host}:${cfg.db.port}/${cfg.db.name}",
            cfg.db.user,
            cfg.db.password,
          )
          .locations("classpath:db/migration")
          .load()
          .migrate()
      }.unit
    }

  // Swagger UI routes (served at /docs)
  private val swaggerRoutes: List[sttp.tapir.ztapir.ZServerEndpoint[Any, Any]] =
    SwaggerInterpreter()
      .fromEndpoints[Task](Endpoints.all, "Flashcards API", "1.0.0")

  override def run: ZIO[ZIOAppArgs & Scope, Any, Any] =
    val program =
      for
        cfg <- ZIO.service[AppConfig]
        _   <- runMigrations
        _   <- ZIO.logInfo(s"Starting Flashcards API on port ${cfg.port}")

        // Combine app routes + swagger UI
        appRoutes     = ZioHttpInterpreter().toHttp(Routes.all)
        swaggerHttp   = ZioHttpInterpreter().toHttp(swaggerRoutes)
        httpApp       = (appRoutes ++ swaggerHttp).toHttpApp

        _   <- Server
                 .serve(httpApp)
                 .provide(
                   Server.defaultWith(_.port(cfg.port)),
                 )
      yield ()

    program.provide(
      AppConfig.layer,
      ZLayer.fromZIO(ZIO.service[AppConfig].map(_.db)) >>> FlashcardRepository.layer,
    )
