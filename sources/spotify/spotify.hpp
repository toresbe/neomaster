#include <dbus-c++/dbus.h>
#include "../sources.hpp"
#include "spotify_glue.h"

class SpotifySource;
class SpotifyEventHandler;
class SpotifyTransport;

class SpotifyEventHandler : public IEventHandler {
    private: 
        beosource::Response handle_keypress(const beosource::Event & event);
        beosource::Response handle_source_change(const beosource::Event & event);
    protected:
        SpotifySource * source;
    public:
        beosource::Response handle(const beosource::Event & event);
        SpotifyEventHandler(SpotifySource *source);
};

class SpotifySource: public ISource {
    public:
        uint8_t masterlink_id = beosource::Source::a_mem;
        INeomaster *kernel;
        SpotifyEventHandler *events;
        SpotifyTransport *transport;
        SpotifySource(INeomaster *kernel);
};

class SpotifyDBus
: public org::mpris::MediaPlayer2::Player_proxy,
    public DBus::IntrospectableProxy,
    public DBus::ObjectProxy
{
    public:
        SpotifyDBus(DBus::Connection& connection, const char* path, const char* name)
            : DBus::ObjectProxy(connection, path, name) {
            }
        void Seeked(const int64_t& Position) {}

};

class SpotifyTransport: public ITransport {
    protected:
        SpotifySource *source;
        SpotifyDBus *dbus;
        DBus::BusDispatcher dispatcher;
        DBus::Connection *bus;
    public:
        SpotifyTransport(SpotifySource *source);
        ~SpotifyTransport();
        void play();
        void stop();
        void prev();
        void next();
};
