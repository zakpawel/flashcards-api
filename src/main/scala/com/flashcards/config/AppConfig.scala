package com.flashcards.config

import zio.*
import zio.config.*
import zio.config.magnolia.*
import zio.config.typesafe.*

final case class DbConfig(
    host: String,
    port: Int,
    name: String,
    user: String,
    password: String,
)

final case class AppConfig(
    db: DbConfig,
    port: Int,
)

object AppConfig:
  val layer: ZLayer[Any, Config.Error, AppConfig] =
    ZLayer.fromZIO(
      read(
        deriveConfig[AppConfig].from(
          ConfigProvider.fromEnv(pathDelim = '_', seqDelim = ',').orElse(
            ConfigProvider.fromResourcePath
          )
        )
      )
    )
