package com.flashcards.domain

import zio.json.*
import java.time.Instant
import java.util.UUID

// ── Model ────────────────────────────────────────────────────────────────────

final case class Flashcard(
    id: UUID,
    front: String,
    back: String,
    category: Option[String],
    createdAt: Instant,
    updatedAt: Instant,
)

final case class CreateFlashcard(
    front: String,
    back: String,
    category: Option[String],
)

final case class UpdateFlashcard(
    front: Option[String],
    back: Option[String],
    category: Option[Option[String]],
)

// ── ZIO JSON codecs ───────────────────────────────────────────────────────────

object Flashcard:
  given JsonEncoder[UUID]       = JsonEncoder[String].contramap(_.toString)
  given JsonDecoder[UUID]       = JsonDecoder[String].map(UUID.fromString)
  given JsonEncoder[Instant]    = JsonEncoder[String].contramap(_.toString)
  given JsonDecoder[Instant]    = JsonDecoder[String].map(Instant.parse)
  given JsonEncoder[Flashcard]  = DeriveJsonEncoder.gen
  given JsonDecoder[Flashcard]  = DeriveJsonDecoder.gen

object CreateFlashcard:
  given JsonEncoder[CreateFlashcard] = DeriveJsonEncoder.gen
  given JsonDecoder[CreateFlashcard] = DeriveJsonDecoder.gen

object UpdateFlashcard:
  given JsonEncoder[UpdateFlashcard] = DeriveJsonEncoder.gen
  given JsonDecoder[UpdateFlashcard] = DeriveJsonDecoder.gen

// ── Error ─────────────────────────────────────────────────────────────────────

enum FlashcardError:
  case NotFound(id: UUID)
  case DatabaseError(cause: Throwable)

object FlashcardError:
  given JsonEncoder[UUID] = JsonEncoder[String].contramap(_.toString)
  given JsonEncoder[FlashcardError] = DeriveJsonEncoder.gen
