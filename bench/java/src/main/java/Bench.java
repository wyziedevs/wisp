// Vert.x serving the same /fortunes, /plaintext, /json and /page as bench/app:
// one verticle per event loop, THREADS of each, sharing the port. The page
// is built with a StringBuilder (no template engine, as Vert.x's TechEmpower
// entry does it) and the JSON with Vert.x's JsonObject (Jackson). It also
// answers the-benchmarker's GET /, GET /user/:id and POST /user as its entry
// there does; `activej` runs ActiveJ instead (ActiveJ.java).
//
//   java -jar target/bench.jar [activej]      PORT sets the port, THREADS the event loops

import io.vertx.core.AbstractVerticle;
import io.vertx.core.DeploymentOptions;
import io.vertx.core.Promise;
import io.vertx.core.Vertx;
import io.vertx.core.VertxOptions;
import io.vertx.core.http.HttpServerRequest;
import io.vertx.core.http.HttpServerResponse;
import io.vertx.core.json.JsonObject;
import java.util.ArrayList;
import java.util.List;

public final class Bench extends AbstractVerticle {
    record Fortune(int id, String message) {}

    static final List<Fortune> ROWS = List.of(
        new Fortune(1, "fortune: No such file or directory"),
        new Fortune(2, "A computer scientist is someone who fixes things that aren't broken."),
        new Fortune(3, "After enough decimal places, nobody gives a damn."),
        new Fortune(4, "A bad random number generator: 1, 1, 1, 1, 1, 4.33e+67, 1, 1, 1"),
        new Fortune(5, "A computer program does what you tell it to do, not what you want it to do."),
        new Fortune(6, "Emacs is a nice operating system, but I prefer UNIX. — Tom Christaensen"),
        new Fortune(7, "Any program that runs right is obsolete."),
        new Fortune(8, "A list is only as strong as its weakest link. — Donald Knuth"),
        new Fortune(9, "Feature: A bug with seniority."),
        new Fortune(10, "Computers make very fast, very accurate mistakes."),
        new Fortune(11, "<script>alert(\"This should not be displayed in a browser alert box.\");</script>"),
        new Fortune(12, "フレームワークのベンチマーク"));

    static final int PORT = Integer.parseInt(System.getenv("PORT"));
    static final int THREADS = System.getenv("THREADS") == null
        ? Runtime.getRuntime().availableProcessors()
        : Integer.parseInt(System.getenv("THREADS"));

    public static void main(String[] args) throws Exception {
        if (args.length > 0 && args[0].equals("activej")) {
            new ActiveJ().launch(args);
            return;
        }
        Vertx vertx = Vertx.vertx(new VertxOptions().setEventLoopPoolSize(THREADS).setPreferNativeTransport(true));
        if (!vertx.isNativeTransportEnabled()) {
            System.err.println("vertx: no native transport, using Java NIO");
        }
        vertx.deployVerticle(Bench::new, new DeploymentOptions().setInstances(THREADS))
            .onFailure(e -> {
                e.printStackTrace();
                System.exit(1);
            });
    }

    @Override
    public void start(Promise<Void> started) {
        vertx.createHttpServer()
            .requestHandler(Bench::handle)
            .listen(PORT, "127.0.0.1")
            .onSuccess(s -> started.complete())
            .onFailure(started::fail);
    }

    static void handle(HttpServerRequest req) {
        HttpServerResponse res = req.response();
        switch (req.path()) {
            case "/plaintext" -> res.putHeader("Content-Type", "text/plain; charset=utf-8").end("Hello, World!");
            case "/fortunes" -> res.putHeader("Content-Type", "text/html; charset=utf-8").end(fortunes());
            case "/json" -> res.putHeader("Content-Type", "application/json")
                .end(new JsonObject().put("message", "Hello, World!").toBuffer());
            case "/page" -> res.putHeader("Content-Type", "text/html; charset=utf-8").end(page());
            case "/", "/user" -> res.end();
            default -> {
                if (req.path().startsWith("/user/")) {
                    res.end(req.path().substring(6));
                } else {
                    res.setStatusCode(404).end();
                }
            }
        }
    }

    static final String[] NAMES = {"Ada <&\"", "Alan <&\"", "Grace <&\"", "Linus <&\"", "Edsger <&\""};

    // /page: a layout, 50 rows built per request with a name to escape and a
    // class chosen by a boolean, and a form.
    static String page() {
        StringBuilder html = new StringBuilder(6000);
        html.append("<!DOCTYPE html>\n<html>\n<head><title>Roster</title></head>\n<body>\n")
            .append("<header><nav><a href=\"/\">Home</a><a href=\"/page\">Roster</a><a href=\"/about\">About</a></nav></header>\n")
            .append("<main>\n<h1>Roster</h1>\n<table>\n<thead><tr><th>id</th><th>name</th><th>score</th></tr></thead>\n<tbody>\n");
        for (int id = 1; id <= 50; id++) {
            html.append("<tr class=\"").append(id % 3 != 0 ? "on" : "off").append("\"><td>").append(id).append("</td><td>");
            escape(html, NAMES[id % 5]);
            html.append("</td><td>").append(id * 37 % 101).append("</td></tr>\n");
        }
        return html.append("</tbody>\n</table>\n")
            .append("<form method=\"post\" action=\"/subscribe\"><label>Email <input type=\"email\" name=\"email\" required></label><button>Subscribe</button></form>\n")
            .append("</main>\n<footer><p>Built with the framework under test.</p></footer>\n</body>\n</html>\n").toString();
    }

    static String fortunes() {
        List<Fortune> list = new ArrayList<>(ROWS.size() + 1);
        list.addAll(ROWS);
        list.add(new Fortune(0, "Additional fortune added at request time."));
        list.sort((a, b) -> a.message().compareTo(b.message()));
        StringBuilder html = new StringBuilder(1200);
        html.append("<!DOCTYPE html>\n<html>\n<head><title>Fortunes</title></head>\n<body><table>\n<tr><th>id</th><th>message</th></tr>\n");
        for (Fortune f : list) {
            html.append("<tr><td>").append(f.id()).append("</td><td>");
            escape(html, f.message());
            html.append("</td></tr>\n");
        }
        return html.append("</table></body>\n</html>\n").toString();
    }

    static void escape(StringBuilder out, String s) {
        for (int i = 0; i < s.length(); i++) {
            char c = s.charAt(i);
            switch (c) {
                case '&' -> out.append("&amp;");
                case '<' -> out.append("&lt;");
                case '>' -> out.append("&gt;");
                case '"' -> out.append("&quot;");
                case '\'' -> out.append("&#39;");
                default -> out.append(c);
            }
        }
    }
}
