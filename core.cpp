#include <signal.h>
#include <boost/log/trivial.hpp>
#include "libpc2/pc2/pc2.hpp"
#include "libpc2/pc2/beo4.hpp"

class BM5PC2Interface: public PC2Interface {
    public:
    BM5PC2Interface() {
        this->address_mask = PC2Interface::address_mask_t::audio_master;
    }
    void beo4_press(Beo4::keycode keycode) {
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
            this->pc2->device->send_message({0xe3, 0xa2, 0xff, 0x00, 0x00});
            this->pc2->device->send_message({0xe5, 0x04, 0x00, 0x00, 0x00});
            this->pc2->device->send_message({0xe4, 0x01});
            this->pc2->mixer->speaker_power(true);
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
            this->pc2->mixer->speaker_power(false);
            this->pc2->device->send_message({0xe5, 0x00, 0x00, 0x00, 0x01});
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
