package com.flashcards

import cask.*
import com.augustnagro.magnum.*
import com.zaxxer.hikari.{HikariConfig, HikariDataSource}
import org.flywaydb.core.Flyway
import ujson.*

import java.time.Instant
import java.util.UUID

// ── Model ─────────────────────────────────────────────────────────────────────

@Table(PostgresDbType, SqlNameMapper.CamelToSnakeCase)
case class Flashcard(
    @Id id: UUID,
    front: String,
    back: String,
    category: Option[String],
    createdAt: Instant,
    updatedAt: Instant,
) derives DbCodec

case class CreateFlashcard(front: String, back: String, category: Option[String])
case class UpdateFlashcard(front: Option[String], back: Option[String], category: Option[String])

// ── JSON helpers ──────────────────────────────────────────────────────────────

def flashcardJson(f: Flashcard): Obj =
  Obj(
    "id"        -> f.id.toString,
    "front"     -> f.front,
    "back"      -> f.back,
    "category"  -> f.category.fold(Null: Value)(Str(_)),
    "createdAt" -> f.createdAt.toString,
    "updatedAt" -> f.updatedAt.toString,
  )

// ── Database setup ────────────────────────────────────────────────────────────

object Db:
  private val cfg = HikariConfig()
  cfg.setJdbcUrl(
    sys.env.getOrElse(
      "DATABASE_URL",
      s"jdbc:postgresql://${sys.env.getOrElse("DB_HOST", "localhost")}:${sys.env.getOrElse("DB_PORT", "5432")}/${sys.env.getOrElse("DB_NAME", "flashcards")}?user=${sys.env.getOrElse("DB_USER", "postgres")}&password=${sys.env.getOrElse("DB_PASSWORD", "postgres")}"
    )
  )
  cfg.setMaximumPoolSize(10)
  val ds = HikariDataSource(cfg)

  def migrate(): Unit =
    Flyway.configure()
      .dataSource(ds)
      .locations("classpath:db/migration")
      .load()
      .migrate()

// ── API ───────────────────────────────────────────────────────────────────────

object Main extends cask.MainRoutes:

  override def host: String = "0.0.0.0"
  override def port: Int    = sys.env.getOrElse("PORT", "8080").toInt

  Db.migrate()

  // ── GET /api/v1/flashcards?category=xxx ────────────────────────────────────
  @cask.get("/api/v1/flashcards")
  def listFlashcards(category: String = ""): Response[String] =
    val cards = connect(Db.ds) {
      if category.isEmpty then
        sql"SELECT * FROM flashcards ORDER BY created_at DESC".query[Flashcard].run()
      else
        sql"SELECT * FROM flashcards WHERE category = $category ORDER BY created_at DESC"
          .query[Flashcard].run()
    }
    Response(
      Arr(cards.map(flashcardJson)*).toString,
      headers = Seq("Content-Type" -> "application/json"),
    )

  // ── GET /api/v1/flashcards/:id ─────────────────────────────────────────────
  @cask.get("/api/v1/flashcards/:id")
  def getFlashcard(id: String): Response[String] =
    val uuid = UUID.fromString(id)
    connect(Db.ds) {
      sql"SELECT * FROM flashcards WHERE id = $uuid".query[Flashcard].runOption()
    } match
      case Some(f) =>
        Response(flashcardJson(f).toString, headers = Seq("Content-Type" -> "application/json"))
      case None =>
        Response(Obj("error" -> s"Flashcard $id not found").toString, statusCode = 404,
          headers = Seq("Content-Type" -> "application/json"))

  // ── POST /api/v1/flashcards ────────────────────────────────────────────────
  @cask.postJson("/api/v1/flashcards")
  def createFlashcard(front: String, back: String, category: String = ""): Response[String] =
    val cat = if category.isEmpty then None else Some(category)
    val card = connect(Db.ds) {
      sql"""
        INSERT INTO flashcards (front, back, category)
        VALUES ($front, $back, $cat)
        RETURNING *
      """.query[Flashcard].run().head
    }
    Response(
      flashcardJson(card).toString,
      statusCode = 201,
      headers = Seq("Content-Type" -> "application/json"),
    )

  // ── PATCH /api/v1/flashcards/:id ───────────────────────────────────────────
  @cask.patchJson("/api/v1/flashcards/:id")
  def updateFlashcard(
      id: String,
      front: Option[String] = None,
      back: Option[String] = None,
      category: Option[String] = None,
  ): Response[String] =
    val uuid = UUID.fromString(id)
    connect(Db.ds) {
      sql"SELECT * FROM flashcards WHERE id = $uuid".query[Flashcard].runOption()
    } match
      case None =>
        Response(Obj("error" -> s"Flashcard $id not found").toString, statusCode = 404,
          headers = Seq("Content-Type" -> "application/json"))
      case Some(existing) =>
        val newFront    = front.getOrElse(existing.front)
        val newBack     = back.getOrElse(existing.back)
        val newCategory = category.orElse(existing.category)
        val updated = connect(Db.ds) {
          sql"""
            UPDATE flashcards
            SET front = $newFront, back = $newBack, category = $newCategory, updated_at = now()
            WHERE id = $uuid
            RETURNING *
          """.query[Flashcard].run().head
        }
        Response(flashcardJson(updated).toString, headers = Seq("Content-Type" -> "application/json"))

  // ── DELETE /api/v1/flashcards/:id ──────────────────────────────────────────
  @cask.delete("/api/v1/flashcards/:id")
  def deleteFlashcard(id: String): Response[String] =
    val uuid = UUID.fromString(id)
    val rows = connect(Db.ds) {
      sql"DELETE FROM flashcards WHERE id = $uuid".update.run()
    }
    if rows > 0 then Response("", statusCode = 204)
    else Response(Obj("error" -> s"Flashcard $id not found").toString, statusCode = 404,
      headers = Seq("Content-Type" -> "application/json"))

  // ── GET / – health check ───────────────────────────────────────────────────
  @cask.get("/")
  def health(): String = "Flashcards API is running"

  initialize()
