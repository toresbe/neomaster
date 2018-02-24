#include <boost/thread.hpp>
#include "modules/modules.hpp"
#include "ui/ui.hpp"

int main(void) {
    UI ui;
    ModuleManager manager;
    for( auto module: manager.get_ui_modules() ) 
        ui.register_ui_module(module);
    ui.start_gui();
    boost::thread ui_thread(boost::bind(&UI::event_loop, &ui));

    ui_thread.join();
    return 0;
}
