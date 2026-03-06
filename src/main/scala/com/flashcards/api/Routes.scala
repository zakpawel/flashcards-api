package com.flashcards.api

import com.flashcards.domain.*
import com.flashcards.repository.FlashcardRepository
import sttp.tapir.ztapir.*
import zio.*

import java.util.UUID

// ── Server-side route implementations ────────────────────────────────────────

object Routes:

  private def repoError(t: Throwable): ErrorResponse =
    ErrorResponse(t.getMessage.nn)

  val getAll: ZServerEndpoint[FlashcardRepository, Any] =
    Endpoints.getAll.zServerLogic { categoryOpt =>
      ZIO.serviceWithZIO[FlashcardRepository] { repo =>
        categoryOpt
          .fold(repo.getAll)(repo.getByCategory)
          .mapError(repoError)
      }
    }

  val getById: ZServerEndpoint[FlashcardRepository, Any] =
    Endpoints.getById.zServerLogic { id =>
      ZIO.serviceWithZIO[FlashcardRepository] { repo =>
        repo
          .getById(id)
          .mapError(repoError)
          .flatMap:
            case Some(fc) => ZIO.succeed(fc)
            case None     => ZIO.fail(ErrorResponse(s"Flashcard $id not found"))
      }
    }

  val create: ZServerEndpoint[FlashcardRepository, Any] =
    Endpoints.create.zServerLogic { cmd =>
      ZIO.serviceWithZIO[FlashcardRepository](_.create(cmd).mapError(repoError))
    }

  val update: ZServerEndpoint[FlashcardRepository, Any] =
    Endpoints.update.zServerLogic { (id, cmd) =>
      ZIO.serviceWithZIO[FlashcardRepository] { repo =>
        repo
          .update(id, cmd)
          .mapError(repoError)
          .flatMap:
            case Some(fc) => ZIO.succeed(fc)
            case None     => ZIO.fail(ErrorResponse(s"Flashcard $id not found"))
      }
    }

  val delete: ZServerEndpoint[FlashcardRepository, Any] =
    Endpoints.delete.zServerLogic { id =>
      ZIO.serviceWithZIO[FlashcardRepository] { repo =>
        repo
          .delete(id)
          .mapError(repoError)
          .flatMap:
            case true  => ZIO.unit
            case false => ZIO.fail(ErrorResponse(s"Flashcard $id not found"))
      }
    }

  val all: List[ZServerEndpoint[FlashcardRepository, Any]] =
    List(getAll, getById, create, update, delete)
