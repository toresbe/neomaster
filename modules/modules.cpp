#include "modules/modules.hpp"
#include "test.hpp"
#include <iostream>
#include <boost/log/trivial.hpp>

namespace Modules {
	Manager::Manager() {
		this->module_list.push_front(new Modules::Test::Module);
		this->module_list.push_front(new Modules::Test::Module);
		this->module_list.push_front(new Modules::Test::Module);
	}

	void NeomasterModuleUI::attach_ui(GUI::NeomasterUI * ui)
	{
		this->ui = ui;
		this->initialize();
	}
}
