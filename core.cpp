#include "sources/spotify/spotify.hpp"
#include <signal.h>
#include <boost/log/trivial.hpp>
#include "libpc2/pc2/pc2.hpp"
#include "libpc2/pc2/beo4.hpp"
#include "libpc2/masterlink/audio_master.hpp"
#include "HTTPRequest.hpp"
#include "beosource.pb.h"

class ConbeeInterface {
    private:
        const std::string token="automation";
        const std::string endpoint="housepi.local";
        const std::string API_URL="http://" + endpoint + "/api/" + token;

        std::time_t light_mode_timeout = std::time(nullptr);

        bool is_in_light_mode() {
            return (std::time(nullptr) - light_mode_timeout < 20);
        }

        void set_light_mode() {
            light_mode_timeout = std::time(nullptr);
        }

        void recall(Beo4::keycode keycode) {
            set_light_mode();
            http::Request request{API_URL + "/groups/5/scenes/" + std::to_string(keycode) + "/recall" };
            const auto response = request.send("PUT");
        }
    public:
        bool handle_keycode(Beo4::keycode keycode) {
            if(keycode == Beo4::keycode::light) {
                set_light_mode();
                return true;
            }

            if(is_in_light_mode() && keycode <= 9) {
                recall(keycode);
                return true;
            }

            return false;
        }
};

class BM5PC2Interface: public PC2Interface, public INeomaster {
    private:
    public:
        SpotifySource spotify;
        ConbeeInterface light;

    BM5PC2Interface() : spotify(this) {
        this->address_mask = PC2Interface::address_mask_t::audio_master;
    }

    AudioMaster * audio_master() {
        return this->pc2->beolink->audio_master;
    }

    void change_source(uint8_t new_source) {
        beosource::Event NewEvent;
        auto source_change = NewEvent.mutable_source_change();
        source_change->set_old_source((beosource::Source_MLSource)active_source);
        active_source = new_source;
        source_change->set_new_source((beosource::Source_MLSource)active_source);
        spotify.events->handle(NewEvent);
    }

    void send_key_to_source(Beo4::keycode keycode) {
        if (active_source != spotify.masterlink_id) return;
        beosource::Event NewEvent;
        auto KeypressEvent = NewEvent.mutable_key_press();
        KeypressEvent->set_keycode((beosource::KeyPress_Beo4Code)keycode);
        spotify.events->handle(NewEvent);
    }

    // Masterlink audio master hooks: a link room can ask for our Spotify source (A.MEM)
    bool ml_start_source(uint8_t src) override {
        if (src != spotify.masterlink_id) return false;
        if (active_source != src) change_source(src);
        return true;
    }

    void ml_stop_source() override {
        // Rooms stopped listening; keep playing if it is also playing locally
        if (active_source == spotify.masterlink_id && !this->pc2->mixer->state.transmitting_locally) {
            spotify.transport->stop();
            active_source = 0;
        }
    }

    void ml_source_key(uint8_t src, uint8_t key) override {
        if (src == active_source) send_key_to_source((Beo4::keycode)key);
    }

    void beo4_press(Beo4::keycode keycode) {
        if (light.handle_keycode(keycode)) return;

        send_key_to_source(keycode);

        if (keycode == Beo4::keycode::cd) {
            this->pc2->device->send_message({0xe4, 0x01});
            this->pc2->mixer->set_parameters(34, -1, 0, 0, false);
            this->pc2->mixer->transmit_locally(true);
            this->pc2->mixer->speaker_power(true);

            change_source(beosource::Source::a_mem);
            // Link rooms already listening to us follow the new source
            audio_master()->set_local_source(beosource::Source::a_mem);
        }

        if (keycode == Beo4::keycode::tv) {
            this->pc2->device->send_message({ 0x24 });
            // Ask the video master for TV. The audio master answers its LINK_INFO and AUDIO_BUS
            // questions and turns on playback from the link when the video master confirms.
            audio_master()->listen_to_video_source(Masterlink::source::tv);
            this->pc2->mixer->set_parameters(34, -1, 0, 0, false);
            this->pc2->mixer->transmit_locally(false);
            this->pc2->mixer->speaker_power(true);
            this->pc2->device->send_message({0xe4, 0x01});
        }
        // Volume up
        else if (keycode == 0x60) {
            this->pc2->mixer->adjust_volume(1);
        }
        // Volume down
        else if (keycode == 0x64) {
            this->pc2->mixer->adjust_volume(-1);
        }
        else if (keycode == 0x0c) {
            // Local standby, in B&O's order: speakers off first, then routing. The audio master
            // releases the video master's source; link rooms still listening keep our source.
            this->pc2->device->send_message({ 0x24 });
            this->pc2->device->send_message({0xe4, 0x01});
            this->pc2->mixer->speaker_power(false);
            this->pc2->mixer->transmit_locally(false);
            audio_master()->local_standby();
            if (active_source == spotify.masterlink_id && audio_master()->listeners().empty()) {
                spotify.transport->stop();
                active_source = 0;
            }
        }
    }
};

static volatile bool keepRunning = 1;

void intHandler(int dummy) {
    dummy = 0; // just to quench the unused variable warning
    keepRunning = 0;
}

int main(int argc, char *argv[]) {
    BM5PC2Interface iface;
    PC2 pc(&iface);
    // std::ref: bind to iface itself, not to a copy of it
    iface.pc2->keystroke_callback = std::bind(&BM5PC2Interface::beo4_press, std::ref(iface), std::placeholders::_1);
    pc.open();

    signal(SIGINT, intHandler);
    signal(SIGTERM, intHandler);
    pc.event_loop(keepRunning);
    pc.mixer->ml_distribute(false);
    pc.mixer->transmit_locally(false);
    pc.mixer->transmit_from_ml(false);
    pc.mixer->speaker_power(false);
    pc.device->close();
}
