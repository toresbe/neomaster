#include "sources/spotify/spotify.hpp"
#include <signal.h>
#include <boost/log/trivial.hpp>
#include "libpc2/pc2/pc2.hpp"
#include "libpc2/pc2/beo4.hpp"
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

    void beo4_press(Beo4::keycode keycode) {
        if (light.handle_keycode(keycode)) return;

        if (active_source == spotify.masterlink_id) {
            beosource::Event NewEvent;
            auto KeypressEvent = NewEvent.mutable_key_press();
            KeypressEvent->set_keycode((beosource::KeyPress_Beo4Code)keycode);
            spotify.events->handle(NewEvent);
        } 

        if (keycode == Beo4::keycode::cd) {
            this->pc2->device->send_message({0xe4, 0x01});
            this->pc2->mixer->set_parameters(34, -1, 0, 0, false);
            this->pc2->mixer->transmit_locally(true);
            this->pc2->mixer->speaker_power(true);

            beosource::Event NewEvent;
            auto source_change = NewEvent.mutable_source_change();
            source_change->set_old_source((beosource::Source_MLSource)active_source);
            active_source = beosource::Source::a_mem;
            source_change->set_new_source((beosource::Source_MLSource)active_source);

            spotify.events->handle(NewEvent);
        }

        if (keycode == Beo4::keycode::tv) {
            // TODO: Use new API, don't send hard-coded hex anymore
            this->pc2->device->send_message({ 0x24 });
            this->pc2->device->send_message({ 0xe0, 0xc0, 0xc1, 0x01, 0x0b, 0x00, 0x00, \
                    0x00, 0x04, 0x03, 0x04, 0x01, 0x01, 0x00, \
                    0x9a, 0x00 });
            this->pc2->device->send_message({ 0xe0, 0xc0, 0xc1, 0x01, 0x0b, 0x00, 0x00, 0x00, \
                    0x45, 0x07, 0x01, 0x02, 0x0b, 0x00, 0x02, 0x01, \
                    0x00, 0x00, 0xea, 0x00 });
            this->pc2->device->send_message({ 0xe0, 0xc0, 0xc1, 0x01, 0x14, 0x00, 0x00, 0x00, \
                    0x04, 0x03, 0x04, 0x01, 0x01, 0x01, 0xa4, 0x00});
            this->pc2->device->send_message({ 0xe0, 0xc0, 0xc1, 0x01, 0x14, 0x00, 0x00, 0x00, \
                    0x08, 0x00, 0x04, 0xa2, 0x00 });
            this->pc2->mixer->set_parameters(34, -1, 0, 0, false);
            this->pc2->mixer->transmit_from_ml(true);
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
            // TODO: Replace with this->pc2->shutdown
            this->pc2->device->send_message({0xe0, 0xc0, 0xc1, 0x01, 0x0b, 0x00, 0x00, 0x00, 0x04, 0x03, 0x04, 0x01, 0x01, 0x00, 0x9a, 0x00});
            this->pc2->device->send_message({0xe0, 0xc0, 0xc1, 0x01, 0x0a, 0x00, 0x00, 0x00, 0x11, 0x02, 0x02, 0x01, 0x00, 0xa2, 0x00});
            this->pc2->device->send_message({0xe4, 0x01});
            this->pc2->mixer->ml_distribute(false);
            this->pc2->mixer->transmit_from_ml(false);
            this->pc2->mixer->transmit_locally(false);
            this->pc2->mixer->speaker_power(false);
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
    iface.pc2->keystroke_callback = std::bind(&BM5PC2Interface::beo4_press, iface, std::placeholders::_1);
    pc.open();

    signal(SIGINT, intHandler);
    signal(SIGTERM, intHandler);
    pc.event_loop(keepRunning);
    pc.mixer->ml_distribute(false);
    pc.mixer->transmit_locally(false);
    pc.mixer->transmit_from_ml(false);
    pc.mixer->speaker_power(false);
}
