#include <signal.h>
#include <boost/log/trivial.hpp>
#include "libpc2/pc2/pc2.hpp"
#include "libpc2/pc2/beo4.hpp"

class BeoportPC2Interface: public PC2Interface {
    void beo4_press(uint8_t keycode) {
        if (keycode == BEO4_KEY_TV) {
            this->pc2->device->send_telegram({ 0x24 });
            this->pc2->device->get_data(); // expect 0x24 0x01
            this->pc2->device->send_telegram({ 0xe0, 0xc0, 0xc1, 0x01, 0x0b, 0x00, 0x00, \
                                               0x00, 0x04, 0x03, 0x04, 0x01, 0x01, 0x00, \
                                               0x9a, 0x00 });
            this->pc2->expect_ack();
            this->pc2->device->get_data();
            this->pc2->device->send_telegram({ 0xe0, 0xc0, 0xc1, 0x01, 0x0b, 0x00, 0x00, 0x00, \
                                               0x45, 0x07, 0x01, 0x02, 0x0b, 0x00, 0x02, 0x01, \
                                               0x00, 0x00, 0xea, 0x00 });
            this->pc2->expect_ack();
            this->pc2->device->get_data();
            this->pc2->device->send_telegram({ 0xe0, 0xc0, 0xc1, 0x01, 0x14, 0x00, 0x00, 0x00, \
                                               0x04, 0x03, 0x04, 0x01, 0x01, 0x01, 0xa4, 0x00});
            this->pc2->expect_ack();
            this->pc2->device->get_data();
            this->pc2->device->send_telegram({ 0xe0, 0xc0, 0xc1, 0x01, 0x14, 0x00, 0x00, 0x00, \
                                               0x08, 0x00, 0x04, 0xa2, 0x00 });
            this->pc2->expect_ack();
            this->pc2->device->get_data();
            this->pc2->mixer->transmit_from_ml(true);
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
    }
};

static volatile bool keepRunning = 1;

void intHandler(int dummy) {
    keepRunning = 0;
}

int main(int argc, char *argv[]) {
    if(argc == 2) {
        BeoportPC2Interface iface;
        PC2 pc(&iface);
        pc.open();

        if(argv[1][0] == '1') {
            pc.mixer->transmit_locally(true);
            pc.mixer->speaker_power(true);
        } else if (argv[1][0] == '0') {
            pc.mixer->speaker_power(false);
        } else if (argv[1][0] == 'm') {
//            signal(SIGINT, intHandler);
//            signal(SIGTERM, intHandler);
            pc.init();
            pc.mixer->set_parameters(0x22, 0, 0, 0, false);
            pc.event_loop(keepRunning);
            pc.mixer->transmit_locally(false);
            pc.mixer->speaker_power(false);
        }
    } else {
        printf("Usage: %s (figure out the rest from source)\n", argv[0]);
    }
}
