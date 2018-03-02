#include <iostream>
#include <string>
#include "ui/ui.hpp"
#include "test.hpp"

TestModule::TestModule() {
    this->ui_module = new UI_module;
    this->ui_module->description = "Test module";
	this->ui_module->label = "TEST";
    std::cout << "Instantiated\n";
};
