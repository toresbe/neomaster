#include <boost/thread.hpp>
#include "modules/modules.hpp"
#include "ui/ui.hpp"
#include "SDL.h"

int main(int argc, char *argv[]) {    
    Modules::Manager manager;
	GUI::NeomasterUI ui(manager.module_list);
    ui.start_gui();
    boost::thread ui_thread(boost::bind(&GUI::NeomasterUI::event_loop, &ui));

    ui_thread.join();
    return 0;
}
