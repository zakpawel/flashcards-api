# ── Stage 1: Build ────────────────────────────────────────────────────────────
FROM eclipse-temurin:21-jdk-alpine AS builder
WORKDIR /build

RUN apk add --no-cache bash curl

# Instalacja sbt
RUN curl -fL https://github.com/sbt/sbt/releases/download/v1.10.1/sbt-1.10.1.tgz \
    | tar xz -C /usr/local && ln -s /usr/local/sbt/bin/sbt /usr/local/bin/sbt

# Cache zależności
COPY project/ project/
COPY build.sbt .
RUN sbt update

# Build fat-jar
COPY src/ src/
RUN sbt assembly

# ── Stage 2: Runtime ──────────────────────────────────────────────────────────
FROM eclipse-temurin:21-jre-alpine
WORKDIR /app

COPY --from=builder /build/target/scala-3.3.4/flashcards.jar app.jar

EXPOSE 8080

ENTRYPOINT ["java", "-jar", "app.jar"]
