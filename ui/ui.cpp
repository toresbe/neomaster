#include "ui.hpp"
#include <chrono>
#include "../modules/modules.hpp"

#include <boost/log/trivial.hpp>
#include <boost/thread/thread.hpp>

UI::UI() {
    //this->panel.set_backlight(true);
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
    } catch (std::exception & e) {
        this->panel = nullptr;
    }
}

void UI::process_panel_input(const BS5panel::bs5_message & msg) { 
    
};

void UI::event_loop() {
    while (1) {
        if (this->panel) 
            process_panel_input(this->panel->read_status());
        else 
            boost::this_thread::sleep_for((boost::chrono::milliseconds)100);
        this->gui->render();
    }
}
