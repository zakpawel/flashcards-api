package com.flashcards.repository

import com.flashcards.config.DbConfig
import com.flashcards.domain.*
import natchez.Trace.Implicits.noop
import skunk.*
import skunk.codec.all.*
import skunk.implicits.*
import zio.*
import zio.interop.catz.*

import java.time.Instant
import java.util.UUID

// ── Skunk codecs ──────────────────────────────────────────────────────────────

private object Codecs:
  val instant: Codec[Instant] =
    timestamptz.imap(_.toInstant)(_.atOffset(java.time.ZoneOffset.UTC))

  val flashcard: Codec[Flashcard] =
    (uuid ~ text ~ text ~ text.opt ~ instant ~ instant).imap {
      case id ~ front ~ back ~ category ~ createdAt ~ updatedAt =>
        Flashcard(id, front, back, category, createdAt, updatedAt)
    }(fc => fc.id ~ fc.front ~ fc.back ~ fc.category ~ fc.createdAt ~ fc.updatedAt)

// ── Queries ───────────────────────────────────────────────────────────────────

private object Queries:
  import Codecs.*

  val getAll: Query[Void, Flashcard] =
    sql"""
      SELECT id, front, back, category, created_at, updated_at
      FROM flashcards
      ORDER BY created_at DESC
    """.query(flashcard)

  val getById: Query[UUID, Flashcard] =
    sql"""
      SELECT id, front, back, category, created_at, updated_at
      FROM flashcards
      WHERE id = $uuid
    """.query(flashcard)

  val getByCategory: Query[String, Flashcard] =
    sql"""
      SELECT id, front, back, category, created_at, updated_at
      FROM flashcards
      WHERE category = $text
      ORDER BY created_at DESC
    """.query(flashcard)

  val insert: Query[String ~ String ~ Option[String], Flashcard] =
    sql"""
      INSERT INTO flashcards (front, back, category)
      VALUES ($text, $text, ${text.opt})
      RETURNING id, front, back, category, created_at, updated_at
    """.query(flashcard)

  val update: Query[String ~ String ~ Option[String] ~ UUID, Flashcard] =
    sql"""
      UPDATE flashcards
      SET front      = $text,
          back       = $text,
          category   = ${text.opt},
          updated_at = now()
      WHERE id = $uuid
      RETURNING id, front, back, category, created_at, updated_at
    """.query(flashcard)

  val delete: Command[UUID] =
    sql"DELETE FROM flashcards WHERE id = $uuid".command

// ── Repository trait ──────────────────────────────────────────────────────────

trait FlashcardRepository:
  def getAll: Task[List[Flashcard]]
  def getById(id: UUID): Task[Option[Flashcard]]
  def getByCategory(category: String): Task[List[Flashcard]]
  def create(cmd: CreateFlashcard): Task[Flashcard]
  def update(id: UUID, cmd: UpdateFlashcard): Task[Option[Flashcard]]
  def delete(id: UUID): Task[Boolean]

// ── Live implementation ───────────────────────────────────────────────────────

object FlashcardRepository:

  val layer: ZLayer[DbConfig, Throwable, FlashcardRepository] =
    ZLayer.scoped {
      for
        cfg  <- ZIO.service[DbConfig]
        pool <- Session
                  .pooled[cats.effect.IO](
                    host     = cfg.host,
                    port     = cfg.port,
                    user     = cfg.user,
                    database = cfg.name,
                    password = Some(cfg.password),
                    max      = 10,
                  )
                  .toScopedZIO
      yield LiveFlashcardRepository(pool)
    }

private final class LiveFlashcardRepository(
    pool: Resource[cats.effect.IO, Session[cats.effect.IO]]
) extends FlashcardRepository:

  private def withSession[A](f: Session[cats.effect.IO] => cats.effect.IO[A]): Task[A] =
    pool.use(f).toZIO

  def getAll: Task[List[Flashcard]] =
    withSession(_.execute(Queries.getAll))

  def getById(id: UUID): Task[Option[Flashcard]] =
    withSession(_.option(Queries.getById)(id))

  def getByCategory(category: String): Task[List[Flashcard]] =
    withSession(_.execute(Queries.getByCategory)(category))

  def create(cmd: CreateFlashcard): Task[Flashcard] =
    withSession(_.unique(Queries.insert)(cmd.front ~ cmd.back ~ cmd.category))

  def update(id: UUID, cmd: UpdateFlashcard): Task[Option[Flashcard]] =
    getById(id).flatMap:
      case None => ZIO.none
      case Some(existing) =>
        val newFront    = cmd.front.getOrElse(existing.front)
        val newBack     = cmd.back.getOrElse(existing.back)
        val newCategory = cmd.category.fold(existing.category)(identity)
        withSession(_.option(Queries.update)(newFront ~ newBack ~ newCategory ~ id))

  def delete(id: UUID): Task[Boolean] =
    withSession(_.execute(Queries.delete)(id)).map(_.rowsAffected > 0)
