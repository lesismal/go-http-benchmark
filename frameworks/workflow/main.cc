// The workflow benchmark server: the C++ counterpart of the Go servers under
// frameworks/, answering the same routes on the same kind of ports.
//
// It is sogou/workflow's own HTTP server benchmark,
// benchmark/benchmark-01-http_server.cc, with the one change this benchmark
// needs: it answers with the request body instead of a fixed string, since
// the clients check that the body comes back byte for byte. Everything else
// is that file's: a WFHttpServer per port, poller_threads set from the command
// line (here, one per CPU the process may run on) and every other global
// setting at workflow's default, and a Date and a Content-Type header on
// every response. workflow adds the Content-Length itself.
//
// - `/echo` on 50 benchmark ports, FIRST_PORT to LAST_PORT (config.Ports in
//   config/config.go), answered with the request body.
// - `/init` and `/ps` on the port after the last one, on a server of their
//   own, as frameworks.StartControlServer serves them for the Go servers.
//   There is no Go runtime here to run github.com/lesismal/perf in, so the
//   sampling is done below, as frameworks/axum does it: `/init` starts
//   sampling this process' CPU and resident memory every `PsInterval` and
//   answers with the pid, and `/ps` answers with the samples as
//   perf.PSCounter's JSON - only its `cpu` and `mem[].rss` fields, which are
//   all the clients read. There is no `/debug/pprof`; the clients ask only
//   the Go servers for profiles.
//
// It takes the flags script/benchmark.sh forwards to every server, in Go's
// flag syntax, and exits on one it does not know, as a Go server would.

#include <atomic>
#include <cerrno>
#include <chrono>
#include <csignal>
#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <ctime>
#include <memory>
#include <mutex>
#include <string>
#include <thread>
#include <vector>

#include <netinet/in.h>
#include <netinet/tcp.h>
#include <sys/resource.h>
#include <sys/socket.h>
#include <unistd.h>

#if defined(__APPLE__)
#include <mach/mach.h>
#endif

#include <workflow/WFFacilities.h>
#include <workflow/WFGlobal.h>
#include <workflow/WFHttpServer.h>

// The framework's benchmark ports, which config.Ports in config/config.go
// lists as "11301:11350"; config's tests hold the two to each other.
static const unsigned short FIRST_PORT = 11301;
static const unsigned short LAST_PORT = 11350;

static const char NAME[] = "workflow";

struct Flags
{
	bool nodelay = true;
	bool reuseport = true;
	long long payload = 1024;
	long long mem_limit = 2LL << 30;
};

static Flags flags;

static WFFacilities::WaitGroup wait_group{1};

static void signal_handler(int)
{
	wait_group.done();
}

[[noreturn]] static void usage(const std::string& err)
{
	if (!err.empty())
		fprintf(stderr, "%s\n", err.c_str());
	fprintf(stderr,
		"Usage of %s.server:\n"
		"  -b int\n    \texpected request body size (default 1024)\n"
		"  -m int\n    \tmemory limit, ignored (default 2147483648)\n"
		"  -nodelay\n    \ttcp nodelay (default true)\n"
		"  -reuseport\n    \treuse port (default true)\n", NAME);
	exit(2);
}

[[noreturn]] static void fatal(const std::string& msg)
{
	fprintf(stderr, "%s server: %s\n", NAME, msg.c_str());
	exit(1);
}

// Go's flag syntax for the flags every server defines: -name=value or
// -name value, one dash or two, and a bool flag on its own for true.
static void parse_flags(int argc, char **argv)
{
	for (int i = 1; i < argc; i++)
	{
		std::string arg = argv[i];
		if (arg.compare(0, 2, "--") == 0)
			arg.erase(0, 2);
		else if (arg.compare(0, 1, "-") == 0)
			arg.erase(0, 1);
		else
			usage("unexpected argument \"" + std::string(argv[i]) + "\"");

		std::string name = arg;
		std::string value;
		bool has_value = false;
		size_t eq = arg.find('=');
		if (eq != std::string::npos)
		{
			name = arg.substr(0, eq);
			value = arg.substr(eq + 1);
			has_value = true;
		}

		auto parse_bool = [&]() -> bool {
			if (!has_value || value == "true" || value == "1" || value == "t" ||
				value == "T" || value == "TRUE" || value == "True")
				return true;
			if (value == "false" || value == "0" || value == "f" ||
				value == "F" || value == "FALSE" || value == "False")
				return false;
			usage("invalid boolean value \"" + value + "\" for -" + name);
		};
		auto parse_int = [&]() -> long long {
			if (!has_value)
			{
				if (i + 1 >= argc)
					usage("flag needs an argument: -" + name);
				value = argv[++i];
			}
			char *end;
			errno = 0;
			long long v = strtoll(value.c_str(), &end, 0);
			if (value.empty() || *end || errno)
				usage("invalid value \"" + value + "\" for -" + name);
			return v;
		};

		if (name == "nodelay")
			flags.nodelay = parse_bool();
		else if (name == "reuseport")
			flags.reuseport = parse_bool();
		else if (name == "b")
			flags.payload = parse_int();
		else if (name == "m")
			flags.mem_limit = parse_int();
		else if (name == "h" || name == "help")
			usage("");
		else
			usage("flag provided but not defined: -" + name);
	}
}

// One poller thread per CPU this process may run on: on Linux the affinity
// mask script/env.sh pins the server with, as tokio sizes axum's pool.
static int available_cpus()
{
#if defined(__linux__)
	cpu_set_t set;
	CPU_ZERO(&set);
	if (sched_getaffinity(0, sizeof set, &set) == 0)
	{
		int n = CPU_COUNT(&set);
		if (n > 0)
			return n;
	}
#endif
	long n = sysconf(_SC_NPROCESSORS_ONLN);
	return n > 0 ? (int)n : 1;
}

// WFHttpServer with the two socket options every server here takes from the
// command line: SO_REUSEPORT on the listener unless -reuseport=false, and
// TCP_NODELAY set on each accepted connection whichever way -nodelay asks, as
// frameworks.Listen does for the Go servers. workflow itself sets only
// SO_REUSEADDR and leaves TCP_NODELAY as the kernel has it.
class BenchServer : public WFHttpServer
{
public:
	BenchServer(const struct WFServerParams *params, http_process_t proc) :
		WFHttpServer(params, std::move(proc))
	{
	}

protected:
	int create_listen_fd() override
	{
		int fd = WFHttpServer::create_listen_fd();
		if (fd >= 0 && flags.reuseport)
		{
			int on = 1;
			setsockopt(fd, SOL_SOCKET, SO_REUSEPORT, &on, sizeof on);
		}
		return fd;
	}

	WFConnection *new_connection(int accept_fd) override
	{
		WFConnection *conn = WFHttpServer::new_connection(accept_fd);
		if (conn)
		{
			int on = flags.nodelay ? 1 : 0;
			setsockopt(accept_fd, IPPROTO_TCP, TCP_NODELAY, &on, sizeof on);
		}
		return conn;
	}
};

// HTTP_SERVER_PARAMS_DEFAULT with the limits a Go net/http server does not
// have taken off, so that they do not decide a run: 2000 connections per
// server would refuse most of a 10k-connection run, and a 10s write timeout
// would drop a connection whose client falls behind on reading pipelined
// responses. The idle keep-alive timeout goes to workflow's maximum, 300s.
static struct WFServerParams server_params()
{
	struct WFServerParams params = HTTP_SERVER_PARAMS_DEFAULT;
	params.max_connections = 2000000;
	params.peer_response_timeout = -1;
	params.keep_alive_timeout = -1;
	return params;
}

static void set_date(protocol::HttpResponse *resp)
{
	char timestamp[32];
	time_t now = time(nullptr);
	struct tm tm;
	gmtime_r(&now, &tm);
	strftime(timestamp, sizeof timestamp, "%a, %d %b %Y %H:%M:%S GMT", &tm);
	resp->add_header_pair("Date", timestamp);
}

// The body back, byte for byte. The request and the response belong to the
// same task, so the response can point at the request's body rather than
// copy it: this is the use HttpMessage.h gives append_output_body_nocopy.
static void on_echo(WFHttpTask *task)
{
	protocol::HttpRequest *req = task->get_req();
	protocol::HttpResponse *resp = task->get_resp();

	if (strcmp(req->get_request_uri(), "/echo") != 0)
	{
		resp->set_status_code("404");
		return;
	}

	set_date(resp);
	resp->add_header_pair("Content-Type", "application/octet-stream");

	const void *body;
	size_t size;
	if (req->get_parsed_body(&body, &size))
		resp->append_output_body_nocopy(body, size);
}

// User and system CPU time this process has used, over all its threads.
static double cpu_seconds()
{
	struct rusage usage;
	if (getrusage(RUSAGE_SELF, &usage) != 0)
		return 0;
	return usage.ru_utime.tv_sec + usage.ru_utime.tv_usec / 1e6 +
		   usage.ru_stime.tv_sec + usage.ru_stime.tv_usec / 1e6;
}

// Resident memory now, rather than getrusage's high-water mark.
static unsigned long long rss_bytes()
{
#if defined(__linux__)
	FILE *f = fopen("/proc/self/statm", "r");
	if (!f)
		return 0;
	unsigned long long size = 0, resident = 0;
	int n = fscanf(f, "%llu %llu", &size, &resident);
	fclose(f);
	if (n != 2)
		return 0;
	return resident * (unsigned long long)sysconf(_SC_PAGESIZE);
#elif defined(__APPLE__)
	mach_task_basic_info_data_t info;
	mach_msg_type_number_t count = MACH_TASK_BASIC_INFO_COUNT;
	if (task_info(mach_task_self(), MACH_TASK_BASIC_INFO,
				  (task_info_t)&info, &count) != KERN_SUCCESS)
		return 0;
	return info.resident_size;
#else
	return 0;
#endif
}

// This process' CPU and resident memory, sampled once per interval once
// /init has started it - the numbers gopsutil gives the Go servers'
// perf.PSCounter: CPU as the percent of one core used over the interval, so
// 100 is one core busy, and memory in bytes.
class Sampler
{
public:
	// Starts sampling, once, however many times /init arrives: a client that
	// retried the request can deliver it twice.
	void start(std::chrono::nanoseconds interval)
	{
		if (started.exchange(true))
		{
			fprintf(stderr, "%s server: /init called again; the ps counter is already running\n", NAME);
			return;
		}
		std::thread([this, interval]() {
			double last_cpu = cpu_seconds();
			auto last_tick = std::chrono::steady_clock::now();
			for (;;)
			{
				std::this_thread::sleep_for(interval);
				double cpu = cpu_seconds();
				auto now = std::chrono::steady_clock::now();
				double wall = std::chrono::duration<double>(now - last_tick).count();
				double percent = wall > 0 ? (cpu - last_cpu) / wall * 100 : 0;
				last_cpu = cpu;
				last_tick = now;
				unsigned long long rss = rss_bytes();
				std::lock_guard<std::mutex> lock(mutex);
				cpu_samples.push_back(percent);
				rss_samples.push_back(rss);
			}
		}).detach();
	}

	std::string json()
	{
		std::lock_guard<std::mutex> lock(mutex);
		std::string out = "{\"cpu\":[";
		char buf[64];
		for (size_t i = 0; i < cpu_samples.size(); i++)
		{
			snprintf(buf, sizeof buf, "%s%.6f", i ? "," : "", cpu_samples[i]);
			out += buf;
		}
		out += "],\"mem\":[";
		for (size_t i = 0; i < rss_samples.size(); i++)
		{
			snprintf(buf, sizeof buf, "%s{\"rss\":%llu}", i ? "," : "", rss_samples[i]);
			out += buf;
		}
		out += "]}";
		return out;
	}

private:
	std::atomic<bool> started{false};
	std::mutex mutex;
	std::vector<double> cpu_samples;
	std::vector<unsigned long long> rss_samples;
};

static Sampler sampler;

// config.InitArgs: {"PsInterval": <nanoseconds>}. Anything else, or a zero,
// samples once a second.
static std::chrono::nanoseconds ps_interval(const std::string& body)
{
	size_t key = body.find("\"PsInterval\"");
	if (key != std::string::npos)
	{
		size_t colon = body.find(':', key);
		if (colon != std::string::npos)
		{
			long long ns = strtoll(body.c_str() + colon + 1, nullptr, 10);
			if (ns > 0)
				return std::chrono::nanoseconds(ns);
		}
	}
	return std::chrono::seconds(1);
}

static void on_control(WFHttpTask *task)
{
	protocol::HttpRequest *req = task->get_req();
	protocol::HttpResponse *resp = task->get_resp();
	const char *uri = req->get_request_uri();
	const char *method = req->get_method();

	if (strcmp(uri, "/init") == 0 && strcmp(method, "POST") == 0)
	{
		const void *body = "";
		size_t size = 0;
		req->get_parsed_body(&body, &size);
		sampler.start(ps_interval(std::string((const char *)body, size)));
		resp->append_output_body(std::to_string(getpid()));
	}
	else if (strcmp(uri, "/ps") == 0 && strcmp(method, "GET") == 0)
	{
		resp->add_header_pair("Content-Type", "application/json");
		resp->append_output_body(sampler.json());
	}
	else
		resp->set_status_code("404");
}

int main(int argc, char **argv)
{
	parse_flags(argc, argv);

	std::signal(SIGINT, signal_handler);
	std::signal(SIGTERM, signal_handler);

	int pollers = available_cpus();
	WFGlobalSettings settings = GLOBAL_SETTINGS_DEFAULT;
	settings.poller_threads = pollers;
	WORKFLOW_library_init(&settings);

	fprintf(stderr, "%s server: nodelay=%s, reuseport=%s, payload=%lld, memory limit=%lld (ignored: no GC to limit), pollers=%d, handlers=%d\n",
			NAME, flags.nodelay ? "true" : "false", flags.reuseport ? "true" : "false",
			flags.payload, flags.mem_limit, settings.poller_threads, settings.handler_threads);

	struct WFServerParams params = server_params();

	BenchServer control(&params, on_control);
	if (control.start(AF_INET, LAST_PORT + 1) != 0)
		fatal("control server listen :" + std::to_string(LAST_PORT + 1) + " failed: " + strerror(errno));

	std::vector<std::unique_ptr<BenchServer>> servers;
	for (unsigned port = FIRST_PORT; port <= LAST_PORT; port++)
	{
		std::unique_ptr<BenchServer> server(new BenchServer(&params, on_echo));
		if (server->start(AF_INET, port) != 0)
			fatal("listen :" + std::to_string(port) + " failed: " + strerror(errno));
		servers.push_back(std::move(server));
	}
	fprintf(stderr, "%s server: listening on %d ports, :%d to :%d\n",
			NAME, LAST_PORT - FIRST_PORT + 1, FIRST_PORT, LAST_PORT);

	wait_group.wait();

	// Every server told to stop before waiting on any, so that they wind down
	// together rather than one after another.
	for (auto& server : servers)
		server->shutdown();
	control.shutdown();
	for (auto& server : servers)
		server->wait_finish();
	control.wait_finish();

	fprintf(stderr, "%s server: exit\n", NAME);
	return 0;
}
