#include <iostream>
#include <string>
#include "ui/ui.hpp"
#include "test.hpp"

TestModule::TestModule() {
    this->ui_module = new UI_module;
    this->ui_module->label = "Test module";
    std::cout << "Instantiated\n";
};
