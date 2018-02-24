#include <forward_list>
#include "gui.hpp"
#include "panel.hpp"
#include "../modules/modules.hpp"
#include <string>

#ifndef UI_HPP
#define UI_HPP

class UI {
    public:
        UI();
        void event_loop();
        bool register_ui_module(const UI_module &module);
        void start_gui();
    private:
        void process_panel_input(const BS5panel::bs5_message &msg);
        ui_module_list_t modules{};
        BS5panel    *panel;
        BS5gui      *gui;
};
#endif
