#include "ui.hpp"
#include <chrono>
#include "../modules/modules.hpp"

#include <boost/log/trivial.hpp>
#include <boost/thread/thread.hpp>

UI::UI() {
};

bool UI::register_ui_module(const UI_module &module) {
    this->modules.push_front(module);
    BOOST_LOG_TRIVIAL(info) << "UI registering module: " << module.label;
    return true;
}

void UI::start_gui() {
    this->gui = new BS5gui(this->modules);
    try {
        this->panel = new BS5panel();
        this->panel->set_backlight(true);
        this->gui->panel = this->panel;
    } catch (std::exception & e) {
        this->panel = nullptr;
    }
}

void UI::event_loop() {
    while (this->running) {
        if (this->panel) 
            this->panel->read_status();
        else 
            boost::this_thread::sleep_for((boost::chrono::milliseconds)100);
        this->gui->render();
    }
}
