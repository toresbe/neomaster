#include "spotify.hpp"

SpotifySource::SpotifySource(INeomaster *kernel) {
    this->kernel = kernel;
    this->events = new SpotifyEventHandler(this);
    this->transport = new SpotifyTransport(this);
};

