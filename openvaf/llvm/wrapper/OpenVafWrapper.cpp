#include "llvm/IR/Instructions.h"
//#include "llvm/Support/CrashRecoveryContext.h"
#include <llvm/IR/Attributes.h>
#include <llvm/IR/Function.h>
#include <llvm/Transforms/IPO/PassManagerBuilder.h>

//#include <iostream>
//#include <mutex>
#include <stdlib.h>

using namespace llvm;

extern "C" {

// TODO(JW) LLVM 18+ has added LLVMGetFastMathFlags and LLVMSetFastMathFlags for 
// getting/setting the fast-math flags of an instruction, as well as 
// LLVMCanValueUseFastMathFlags for checking if an instruction can use such flags.
// See if we can use that rather than this wrapper.

/*
LLVMFastMathFlags LLVMGetFastMathFlags(LLVMValueRef FPMathInst)
// Get the flags for which fast-math-style optimizations are allowed for this value.
void LLVMSetFastMathFlags(LLVMValueRef FPMathInst, LLVMFastMathFlags FMF)
// Sets the flags for which fast-math-style optimizations are allowed for this value.
*/

// Enable some fast-math flags for an operation
// These flags are used for derivatives by default because they only change
// the rounding behaviour which is not relevant for automatically generated code
// (derivatives in OpenVAF)
// https://llvm.org/docs/LangRef.html#fast-math-flags
void LLVMSetPartialFastMath(LLVMValueRef V) {
  if (auto I = dyn_cast<Instruction>(unwrap<Value>(V))) {
    I->setHasAllowReassoc(true);
    I->setHasAllowReciprocal(true);
    I->setHasAllowContract(true);
  }
}

// Enable fast-math flags for an operation
// https://llvm.org/docs/LangRef.html#fast-math-flags
void LLVMSetFastMath(LLVMValueRef V) {
  if (auto I = dyn_cast<Instruction>(unwrap<Value>(V))) {
    I->setFast(true);
  }
}

void LLVMPurgeAttrs(LLVMValueRef V) {
  if (auto func = dyn_cast<Function>(unwrap<Value>(V))) {
    func->setAttributes(AttributeList());
  }
}

void LLVMPassManagerBuilderSLPVectorize(LLVMPassManagerBuilderRef PMB) {
  PassManagerBuilder *Builder = unwrap(PMB);
  Builder->SLPVectorize = true;
}
}
