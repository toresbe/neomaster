#include "modules/modules.hpp"
#include "test.hpp"
#include "../ui/ui.hpp"
#include <iostream>
#include <boost/log/trivial.hpp>

namespace Modules {
	Manager::Manager() {
		this->module_list.push_front(new Modules::Test::Module);
		this->module_list.push_front(new Modules::Test::Module);
		this->module_list.push_front(new Modules::Test::Module);
	}

	void NeomasterModuleUI::attach_ui(NeomasterUI * ui)
	{
		this->ui = ui;
		this->initialize();
	}
}