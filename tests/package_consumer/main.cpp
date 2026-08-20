#include <brainstem/client.hpp>

int main() {
  brainstem::Config config;
  config.target = "127.0.0.1:13145";
  config.client_id = "consumer@tests";
  config.allow_insecure = true;
  brainstem::Client client(config);
  return client.state().connected ? 1 : 0;
}
