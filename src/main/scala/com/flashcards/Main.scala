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

// ── JSON helpers ──────────────────────────────────────────────────────────────

def flashcardJson(f: Flashcard): Obj =
  Obj(
    "id"        -> f.id.toString,
    "front"     -> f.front,
    "back"      -> f.back,
    "category"  -> f.category.fold[Value](Null)(Str(_)),
    "createdAt" -> f.createdAt.toString,
    "updatedAt" -> f.updatedAt.toString,
  )

def jsonResponse(v: Value, status: Int = 200): Response[String] =
  Response(v.toString, statusCode = status, headers = Seq("Content-Type" -> "application/json"))

def notFound(id: String): Response[String] =
  jsonResponse(Obj("error" -> s"Flashcard $id not found"), status = 404)

// ── Database setup ────────────────────────────────────────────────────────────

object Db:
  private val jdbcUrl =
    sys.env.getOrElse(
      "DATABASE_URL",
      s"jdbc:postgresql://${sys.env.getOrElse("DB_HOST","localhost")}:" +
      s"${sys.env.getOrElse("DB_PORT","5432")}/" +
      s"${sys.env.getOrElse("DB_NAME","flashcards")}?" +
      s"user=${sys.env.getOrElse("DB_USER","postgres")}&" +
      s"password=${sys.env.getOrElse("DB_PASSWORD","postgres")}"
    )

  private val cfg = HikariConfig()
  cfg.setJdbcUrl(jdbcUrl)
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

  // GET / – health check
  @cask.get("/")
  def health() = "Flashcards API is running"

  // GET /api/v1/flashcards?category=xxx
  @cask.get("/api/v1/flashcards")
  def listFlashcards(category: String = ""): Response[String] =
    val cards = connect(Db.ds) {
      if category.isEmpty then
        sql"SELECT * FROM flashcards ORDER BY created_at DESC".query[Flashcard].run()
      else
        sql"SELECT * FROM flashcards WHERE category = $category ORDER BY created_at DESC"
          .query[Flashcard].run()
    }
    jsonResponse(Arr(cards.map(flashcardJson)*))

  // GET /api/v1/flashcards/:id
  @cask.get("/api/v1/flashcards/:id")
  def getFlashcard(id: String): Response[String] =
    val uuid = UUID.fromString(id)
    connect(Db.ds) {
      sql"SELECT * FROM flashcards WHERE id = $uuid".query[Flashcard].run().headOption
    } match
      case Some(f) => jsonResponse(flashcardJson(f))
      case None    => notFound(id)

  // POST /api/v1/flashcards  { "front": "...", "back": "...", "category": "..." }
  @cask.postJson("/api/v1/flashcards")
  def createFlashcard(front: String, back: String, category: String = ""): Response[String] =
    val cat = Option(category).filter(_.nonEmpty)
    val card = connect(Db.ds) {
      sql"""INSERT INTO flashcards (front, back, category)
            VALUES ($front, $back, $cat)
            RETURNING *""".query[Flashcard].run().head
    }
    jsonResponse(flashcardJson(card), status = 201)

  // PATCH /api/v1/flashcards/:id  – raw body JSON, all fields optional
  @cask.patch("/api/v1/flashcards/:id")
  def updateFlashcard(id: String, request: cask.Request): Response[String] =
    val uuid = UUID.fromString(id)
    val body = ujson.read(request.bytes).obj
    connect(Db.ds) {
      sql"SELECT * FROM flashcards WHERE id = $uuid".query[Flashcard].run().headOption
    } match
      case None => notFound(id)
      case Some(existing) =>
        val newFront    = body.get("front").map(_.str).getOrElse(existing.front)
        val newBack     = body.get("back").map(_.str).getOrElse(existing.back)
        val newCategory = body.get("category") match
          case Some(Null)    => None
          case Some(s: Str)  => Some(s.str)
          case _             => existing.category
        val updated = connect(Db.ds) {
          sql"""UPDATE flashcards
                SET front = $newFront, back = $newBack, category = $newCategory, updated_at = now()
                WHERE id = $uuid
                RETURNING *""".query[Flashcard].run().head
        }
        jsonResponse(flashcardJson(updated))

  // DELETE /api/v1/flashcards/:id
  @cask.delete("/api/v1/flashcards/:id")
  def deleteFlashcard(id: String): Response[String] =
    val uuid = UUID.fromString(id)
    val rows = connect(Db.ds) {
      sql"DELETE FROM flashcards WHERE id = $uuid".update.run()
    }
    if rows > 0 then Response("", statusCode = 204)
    else notFound(id)

  initialize()
