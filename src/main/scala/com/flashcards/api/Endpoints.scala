package com.flashcards.api

import com.flashcards.domain.*
import sttp.tapir.*
import sttp.tapir.generic.auto.*
import sttp.tapir.json.zio.*
import sttp.model.StatusCode

import java.util.UUID

// ── Shared error response ─────────────────────────────────────────────────────

final case class ErrorResponse(message: String)
object ErrorResponse:
  import zio.json.*
  given JsonEncoder[ErrorResponse] = DeriveJsonEncoder.gen
  given JsonDecoder[ErrorResponse] = DeriveJsonDecoder.gen

// ── Endpoint definitions ──────────────────────────────────────────────────────

object Endpoints:

  private val base = endpoint
    .in("api" / "v1" / "flashcards")
    .errorOut(
      oneOf[ErrorResponse](
        oneOfVariant(StatusCode.NotFound,            jsonBody[ErrorResponse]),
        oneOfVariant(StatusCode.InternalServerError, jsonBody[ErrorResponse]),
      )
    )

  val getAll: PublicEndpoint[Option[String], ErrorResponse, List[Flashcard], Any] =
    base.get
      .in(query[Option[String]]("category").description("Filter by category"))
      .out(jsonBody[List[Flashcard]])
      .description("List all flashcards, optionally filtered by category")
      .name("listFlashcards")

  val getById: PublicEndpoint[UUID, ErrorResponse, Flashcard, Any] =
    base.get
      .in(path[UUID]("id").description("Flashcard UUID"))
      .out(jsonBody[Flashcard])
      .description("Get a single flashcard by ID")
      .name("getFlashcard")

  val create: PublicEndpoint[CreateFlashcard, ErrorResponse, Flashcard, Any] =
    base.post
      .in(jsonBody[CreateFlashcard].description("Flashcard to create"))
      .out(statusCode(StatusCode.Created).and(jsonBody[Flashcard]))
      .description("Create a new flashcard")
      .name("createFlashcard")

  val update: PublicEndpoint[(UUID, UpdateFlashcard), ErrorResponse, Flashcard, Any] =
    base.patch
      .in(path[UUID]("id"))
      .in(jsonBody[UpdateFlashcard].description("Fields to update (all optional)"))
      .out(jsonBody[Flashcard])
      .description("Partially update a flashcard")
      .name("updateFlashcard")

  val delete: PublicEndpoint[UUID, ErrorResponse, Unit, Any] =
    base.delete
      .in(path[UUID]("id"))
      .out(statusCode(StatusCode.NoContent))
      .description("Delete a flashcard")
      .name("deleteFlashcard")

  val all = List(getAll, getById, create, update, delete)
