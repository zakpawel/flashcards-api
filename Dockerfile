# Runtime image: static musl binary needs no libc → FROM scratch
FROM scratch
WORKDIR /app

COPY target/x86_64-unknown-linux-musl/release/flashcards flashcards

EXPOSE 8080

ENTRYPOINT ["/app/flashcards"]
