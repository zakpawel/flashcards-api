# ── Runtime ───────────────────────────────────────────────────────────────────
# The fat-jar is built by the CI 'build' job and passed as a build artifact.
# This stage just packages it into a minimal JRE image.
FROM eclipse-temurin:21-jre-alpine
WORKDIR /app

COPY target/scala-3.3.4/flashcards.jar app.jar

EXPOSE 8080

ENTRYPOINT ["java", "-jar", "app.jar"]
