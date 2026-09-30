// ActiveJ: its the-benchmarker entry (java/activej), answering only their
// routes. The multithreaded HTTP launcher, a worker per THREADS, each with
// its own RoutingServlet.
//
//   java -jar target/bench.jar activej      PORT sets the port, THREADS the workers

import io.activej.config.Config;
import io.activej.http.AsyncServlet;
import io.activej.http.HttpResponse;
import io.activej.http.RoutingServlet;
import io.activej.inject.annotation.Provides;
import io.activej.inject.module.AbstractModule;
import io.activej.inject.module.Module;
import io.activej.launchers.http.MultithreadedHttpServerLauncher;
import io.activej.worker.annotation.Worker;
import java.net.InetSocketAddress;

import static io.activej.bytebuf.ByteBufStrings.wrapAscii;
import static io.activej.config.Config.ofSystemProperties;
import static io.activej.config.converter.ConfigConverters.ofInetSocketAddress;

public final class ActiveJ extends MultithreadedHttpServerLauncher {
    @Provides
    @Worker
    AsyncServlet mainServlet() {
        return RoutingServlet.create()
            .map("/", request -> HttpResponse.ok200())
            .map("/user/:id", request -> HttpResponse.ok200().withBody(wrapAscii(request.getPathParameter("id"))))
            .map("/user", request -> HttpResponse.ok200());
    }

    @Override
    protected Module getOverrideModule() {
        return new AbstractModule() {
            @Provides
            Config config() {
                return Config.create()
                    .with("http.listenAddresses", Config.ofValue(ofInetSocketAddress(), new InetSocketAddress("127.0.0.1", Bench.PORT)))
                    .with("workers", String.valueOf(Bench.THREADS))
                    .overrideWith(ofSystemProperties("config"));
            }
        };
    }
}
